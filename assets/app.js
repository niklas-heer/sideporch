// Sideporch's only page script. Pages work without it; it adds live
// updates, sending without a page reload, reactions in place, the emoji
// picker, push notifications, local times, drafts and copy buttons.
"use strict";

(() => {
  const app = document.getElementById("app");
  // Touch keyboards: Enter makes a new line instead of sending.
  const touch = matchMedia("(pointer: coarse)").matches;
  const GROUP_WINDOW_MS = 5 * 60 * 1000;
  const timeFormat = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit" });
  const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "medium" });
  const fullFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "full", timeStyle: "short" });
  const dateTimeFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

  // Sign-up forms carry a proof-of-work challenge: find a nonce whose
  // SHA-256 of "challenge:nonce" starts with enough zero bits. A moment of
  // work for a person, a real cost for bots signing up by the thousand.
  const K = new Uint32Array([
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
  ]);
  // The first word of SHA-256 over ASCII `text`, enough to count zero bits
  // up to 32.
  function sha256FirstWord(text) {
    const length = text.length;
    const blocks = ((length + 9 + 63) >> 6) << 4;
    const words = new Uint32Array(blocks);
    for (let i = 0; i < length; i++) words[i >> 2] |= text.charCodeAt(i) << (24 - (i & 3) * 8);
    words[length >> 2] |= 0x80 << (24 - (length & 3) * 8);
    words[blocks - 1] = length * 8;
    const h = new Uint32Array([0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]);
    const w = new Uint32Array(64);
    for (let block = 0; block < blocks; block += 16) {
      for (let i = 0; i < 16; i++) w[i] = words[block + i];
      for (let i = 16; i < 64; i++) {
        const a = w[i - 15], b = w[i - 2];
        const s0 = ((a >>> 7) | (a << 25)) ^ ((a >>> 18) | (a << 14)) ^ (a >>> 3);
        const s1 = ((b >>> 17) | (b << 15)) ^ ((b >>> 19) | (b << 13)) ^ (b >>> 10);
        w[i] = w[i - 16] + s0 + w[i - 7] + s1;
      }
      let [a, b, c, d, e, f, g, hh] = h;
      for (let i = 0; i < 64; i++) {
        const S1 = ((e >>> 6) | (e << 26)) ^ ((e >>> 11) | (e << 21)) ^ ((e >>> 25) | (e << 7));
        const t1 = (hh + S1 + ((e & f) ^ (~e & g)) + K[i] + w[i]) | 0;
        const S0 = ((a >>> 2) | (a << 30)) ^ ((a >>> 13) | (a << 19)) ^ ((a >>> 22) | (a << 10));
        const t2 = (S0 + ((a & b) ^ (a & c) ^ (b & c))) | 0;
        hh = g; g = f; f = e; e = (d + t1) | 0; d = c; c = b; b = a; a = (t1 + t2) | 0;
      }
      h[0] += a; h[1] += b; h[2] += c; h[3] += d; h[4] += e; h[5] += f; h[6] += g; h[7] += hh;
    }
    return h[0];
  }
  for (const form of document.querySelectorAll("form[data-proof]")) {
    const challenge = form.dataset.proof;
    const bits = Number(form.dataset.proofBits);
    const status = form.querySelector("[data-proof-status]");
    let nonce = 0;
    let solved = false;
    let submitWhenSolved = false;
    const work = () => {
      const until = performance.now() + 30;
      while (performance.now() < until) {
        for (let i = 0; i < 2000; i++, nonce++) {
          if (Math.clz32(sha256FirstWord(`${challenge}:${nonce}`)) >= bits) {
            form.elements.proof.value = String(nonce);
            solved = true;
            if (status) status.textContent = "";
            if (submitWhenSolved) form.requestSubmit();
            return;
          }
        }
      }
      setTimeout(work, 0);
    };
    work();
    form.addEventListener("submit", (event) => {
      if (solved) return;
      event.preventDefault();
      submitWhenSolved = true;
      if (status) status.textContent = "Checking that you're not a bot…";
    });
  }

  function localizeTimes(root) {
    for (const time of root.querySelectorAll("time[datetime]")) {
      const date = new Date(time.getAttribute("datetime"));
      if (Number.isNaN(date.getTime())) continue;
      const format = { date: dateFormat, datetime: dateTimeFormat }[time.dataset.format] || timeFormat;
      time.textContent = format.format(date);
      time.title = fullFormat.format(date);
    }
  }

  // Mermaid is large, so it loads only when a page shows a diagram.
  let mermaid = null;
  function drawDiagrams(root) {
    const blocks = [...root.querySelectorAll("pre.mermaid:not([data-processed])")];
    if (blocks.length === 0) return;
    mermaid ||= new Promise((resolve, reject) => {
      const script = document.createElement("script");
      script.src = "/assets/mermaid.js?v=12.0.0";
      script.onload = () => {
        const dark = document.documentElement.classList.contains("dark");
        globalThis.mermaid.initialize({ startOnLoad: false, securityLevel: "strict", theme: dark ? "dark" : "neutral" });
        resolve(globalThis.mermaid);
      };
      script.onerror = reject;
      document.head.append(script);
    });
    mermaid
      .then((library) => library.run({ nodes: blocks, suppressErrors: true }))
      .catch((error) => console.warn("sideporch: diagrams unavailable", error));
  }

  function scrollToEnd(scroller) {
    if (scroller) scroller.scrollTop = scroller.scrollHeight;
  }

  function nearEnd(scroller) {
    return !scroller || scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 120;
  }

  // Appends a server-rendered message, grouping it under the previous one
  // when the same author wrote both within a few minutes.
  function appendMessage(list, html, scroller) {
    const template = document.createElement("template");
    template.innerHTML = html.trim();
    const item = template.content.firstElementChild;
    if (!item || document.getElementById(item.id)) return;
    const previous = list.lastElementChild;
    if (
      previous &&
      previous.dataset.author === item.dataset.author &&
      Number(item.dataset.created) - Number(previous.dataset.created) < GROUP_WINDOW_MS
    ) {
      item.dataset.compact = "";
    }
    const follow = nearEnd(scroller);
    localizeTimes(item);
    markOwnReactions(item);
    drawDiagrams(item);
    list.append(item);
    if (follow) scrollToEnd(scroller);
  }

  // Reactions and polls are rendered once for everyone; mark the viewer's
  // own reactions, votes and ranking, and offer to end their own polls.
  function markOwnReactions(root) {
    const me = app?.dataset.me;
    for (const button of root.querySelectorAll("[data-users]")) {
      const mine = button.dataset.users.split(",").includes(me);
      button.setAttribute("aria-pressed", mine ? "true" : "false");
    }
    for (const form of root.querySelectorAll("form[data-rank-form]")) {
      let ranking = [];
      try {
        ranking = JSON.parse(form.dataset.ballots || "{}")[me] || [];
      } catch {}
      for (const select of form.querySelectorAll("select")) {
        const option = Number(select.name.slice(1));
        const place = ranking.indexOf(option);
        select.value = place === -1 ? "" : String(place + 1);
      }
      const save = form.querySelector("[data-rank-save]");
      if (save) save.hidden = true;
    }
    for (const close of root.querySelectorAll("form[data-poll-close]")) {
      close.hidden = !(close.dataset.pollClose === me || app?.dataset.admin !== undefined);
    }
  }

  // Ranks save as soon as they change. Giving an option a rank another
  // option has swaps the two, so every rank stays unique.
  document.addEventListener("focusin", (event) => {
    const select = event.target.closest("form[data-rank-form] select");
    if (select) select.dataset.previous = select.value;
  });
  document.addEventListener("change", (event) => {
    const select = event.target.closest("form[data-rank-form] select");
    if (!select) return;
    const form = select.form;
    const previous = select.dataset.previous ?? "";
    if (select.value) {
      for (const other of form.querySelectorAll("select")) {
        if (other !== select && other.value === select.value) other.value = previous;
      }
    }
    select.dataset.previous = select.value;
    fetch(form.action, { method: "POST", headers: { "x-sideporch-fetch": "1" }, body: new URLSearchParams(new FormData(form)) })
      .then((response) => {
        if (!response.ok) toast("Your ranking wasn't saved. Try again.");
      })
      .catch(() => toast("Your ranking wasn't saved. Check your connection."));
  });

  // The poll form posts in the background and closes when the poll is up.
  document.addEventListener("submit", async (event) => {
    const form = event.target.closest("form[data-poll-form]");
    if (!form) return;
    event.preventDefault();
    const error = form.querySelector("[data-poll-error]");
    const response = await fetch(form.action, {
      method: "POST",
      headers: { "x-sideporch-fetch": "1" },
      body: new URLSearchParams(new FormData(form)),
    }).catch(() => null);
    if (response?.ok) {
      form.reset();
      error.classList.add("hidden");
      form.closest("[popover]")?.hidePopover();
      return;
    }
    const page = response ? await response.text() : "";
    const message = new DOMParser().parseFromString(page, "text/html").querySelector("main p, p")?.textContent;
    error.textContent = response ? message || "That poll didn't work. Check it and try again." : "That didn't work. Check your connection.";
    error.classList.remove("hidden");
  });

  function replaceReactions(messageId, html) {
    const current = document.getElementById(`reactions-${messageId}`);
    if (!current) return;
    const template = document.createElement("template");
    template.innerHTML = html.trim();
    const next = template.content.firstElementChild;
    if (!next) return;
    markOwnReactions(next);
    current.replaceWith(next);
  }

  function replyLabel(count) {
    return count === 1 ? "1 reply" : `${count} replies`;
  }

  function updateReplyCount(parentId, count) {
    if (count === null || count === undefined) return;
    if (app.dataset.thread === String(parentId)) {
      const label = document.getElementById("thread-count");
      if (label) label.textContent = replyLabel(count);
    }
    const link = document.querySelector(`[data-reply-count="${parentId}"]`);
    if (!link) return;
    link.classList.toggle("hidden", count === 0);
    link.classList.toggle("inline-flex", count > 0);
    const label = link.querySelector("span");
    if (label) label.textContent = replyLabel(count);
  }

  // Every copy of a message on the page: the channel list and, for the
  // first message of an open thread, the thread panel.
  function messageCopies(id) {
    return [...document.querySelectorAll(`li[id="m${id}"]`)];
  }

  function replaceMessage(id, html) {
    for (const current of messageCopies(id)) {
      // Leave a message alone while someone edits it here.
      if (current.querySelector("form[data-edit]")) continue;
      const template = document.createElement("template");
      template.innerHTML = html.trim();
      const next = template.content.firstElementChild;
      if (!next) return;
      if (current.dataset.compact !== undefined) next.dataset.compact = "";
      // Saved is personal, and live updates are rendered for everyone.
      if (current.dataset.saved !== undefined) next.dataset.saved = "";
      if (current.closest("aside") && !current.closest("#replies")) next.querySelector("[data-reply-count]")?.remove();
      localizeTimes(next);
      markOwnReactions(next);
      drawDiagrams(next);
      current.replaceWith(next);
    }
  }

  function removeMessage(event) {
    if (app.dataset.thread === String(event.id)) {
      location.href = `/c/${event.channel_id}`;
      return;
    }
    for (const current of messageCopies(event.id)) {
      const next = current.nextElementSibling;
      // The next message loses its shared header if it relied on this one.
      if (next?.dataset.compact !== undefined && current.dataset.compact === undefined) delete next.dataset.compact;
      current.remove();
    }
    if (event.parent_id !== null) updateReplyCount(event.parent_id, event.reply_count);
  }

  // Who is writing, per composer on this page.
  const typists = new Map();
  function composerFor(parentId) {
    return [...document.querySelectorAll("form[data-composer]")].find(
      (form) => (form.elements.parent_id?.value ?? "") === String(parentId ?? ""),
    );
  }
  function typingDots() {
    const dots = document.createElement("span");
    dots.className = "typing-dots";
    dots.setAttribute("aria-hidden", "true");
    dots.append(...[0, 1, 2].map(() => document.createElement("i")));
    return dots;
  }
  function renderTyping(form) {
    const label = form.querySelector("[data-typing]");
    if (!label) return;
    const names = [...(typists.get(form)?.values() ?? [])].map((person) => person.name);
    if (names.length === 0) {
      label.replaceChildren();
      return;
    }
    label.replaceChildren(
      typingDots(),
      names.length === 1 ? `${names[0]} is typing…` :
      names.length === 2 ? `${names[0]} and ${names[1]} are typing…` :
      "Several people are typing…",
    );
  }
  // Direct conversations that aren't open show typing in the sidebar.
  const sidebarTypists = new Map();
  function markSidebarTyping(channelId, userId, stopped) {
    const key = `${channelId}:${userId}`;
    clearTimeout(sidebarTypists.get(key));
    sidebarTypists.delete(key);
    const refresh = () => document.querySelector(`[data-channel-link="${channelId}"]`)
      ?.toggleAttribute("data-typing", [...sidebarTypists.keys()].some((other) => other.startsWith(`${channelId}:`)));
    if (!stopped) {
      sidebarTypists.set(key, setTimeout(() => {
        sidebarTypists.delete(key);
        refresh();
      }, 5000));
    }
    refresh();
  }
  function showTyping(event) {
    if (String(event.user_id) === app?.dataset.me) return;
    if (app?.dataset.channel !== String(event.channel_id)) {
      markSidebarTyping(event.channel_id, event.user_id, event.stopped);
      return;
    }
    if (event.stopped) {
      stopTyping(event.user_id, event.parent_id);
      return;
    }
    const form = composerFor(event.parent_id);
    if (!form) return;
    const people = typists.get(form) ?? new Map();
    typists.set(form, people);
    clearTimeout(people.get(event.user_id)?.timer);
    const timer = setTimeout(() => {
      people.delete(event.user_id);
      renderTyping(form);
    }, 5000);
    people.set(event.user_id, { name: event.name, timer });
    renderTyping(form);
  }
  function stopTyping(author, parentId) {
    const form = composerFor(parentId);
    const people = form && typists.get(form);
    const id = Number(String(author).replace(/^u:/, ""));
    if (!people?.has(id)) return;
    clearTimeout(people.get(id).timer);
    people.delete(id);
    renderTyping(form);
  }

  function handleEvent(event, socket) {
    if (event.type === "resync") {
      location.reload();
      return;
    }
    if (event.type === "typing") {
      showTyping(event);
      return;
    }
    if (event.type === "message" && event.activity?.includes(Number(app?.dataset.me)) && location.pathname !== "/activity") {
      const activity = document.querySelector('a[data-nav-link][href="/activity"]');
      if (activity) activity.dataset.unread = "";
    }
    if (event.type === "reactions") {
      replaceReactions(event.message_id, event.html);
      return;
    }
    if (event.type === "message_changed") {
      replaceMessage(event.id, event.html);
      return;
    }
    if (event.type === "message_deleted") {
      removeMessage(event);
      return;
    }
    if (event.type !== "message") return;
    const here = app && app.dataset.channel === String(event.channel_id);
    if (here) stopTyping(event.author, event.parent_id);
    else if (String(event.author).startsWith("u:")) markSidebarTyping(event.channel_id, event.author.slice(2), true);
    if (!here) {
      const link = document.querySelector(`[data-channel-link="${event.channel_id}"]`);
      if (link && link.dataset.muted === undefined && event.author !== `u:${app?.dataset.me}`) {
        link.dataset.unread = "";
        updateAppBadge();
      }
      return;
    }
    if (event.parent_id === null) {
      const list = document.getElementById("messages");
      if (list) appendMessage(list, event.html, document.getElementById("scroller"));
    } else {
      if (app.dataset.thread === String(event.parent_id)) {
        const replies = document.getElementById("replies");
        if (replies) appendMessage(replies, event.html, document.getElementById("thread-scroller"));
      }
      updateReplyCount(event.parent_id, event.reply_count);
    }
    if (document.visibilityState === "visible" && socket.readyState === WebSocket.OPEN) {
      socket.send(JSON.stringify({ type: "read", channel_id: event.channel_id, message_id: event.id }));
    }
  }

  // Live updates. After a dropped connection the page reloads to catch up
  // on anything it missed; drafts survive in session storage.
  let liveSocket = null;
  function connect(attempt = 0) {
    const protocol = location.protocol === "https:" ? "wss:" : "ws:";
    const socket = new WebSocket(`${protocol}//${location.host}/ws`);
    liveSocket = socket;
    let opened = false;
    const sendVisibility = () => {
      if (socket.readyState === WebSocket.OPEN) {
        socket.send(JSON.stringify({ type: "visibility", visible: document.visibilityState === "visible" }));
      }
    };
    socket.addEventListener("open", () => {
      opened = true;
      if (attempt > 0) location.reload();
      sendVisibility();
      // Only the channel on screen needs messages in full.
      socket.send(JSON.stringify({ type: "view", channel_id: app?.dataset.channel ? Number(app.dataset.channel) : null }));
      // Reminders and scheduled messages read times in this zone.
      const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;
      if (zone) socket.send(JSON.stringify({ type: "timezone", name: zone }));
    });
    document.addEventListener("visibilitychange", sendVisibility);
    socket.addEventListener("message", (message) => {
      try {
        handleEvent(JSON.parse(message.data), socket);
      } catch (error) {
        console.error("sideporch: bad event", error);
      }
    });
    socket.addEventListener("close", () => {
      document.removeEventListener("visibilitychange", sendVisibility);
      const delay = Math.min(30000, 1000 * 2 ** Math.min(attempt, 5));
      setTimeout(() => connect(opened ? 1 : attempt + 1), delay);
    });
  }

  // Suggests slash commands while the message is just `/name`.
  let commandList = null;
  function setupCommandSuggestions(form, textarea) {
    const box = document.createElement("ul");
    box.className = "absolute bottom-full left-4 z-10 mb-2 hidden w-full max-w-md overflow-hidden rounded-xl border border-line bg-white py-1 shadow-xl dark:border-night-line dark:bg-night-2";
    box.setAttribute("role", "listbox");
    form.classList.add("relative");
    form.append(box);
    let matches = [];
    let chosen = 0;
    const close = () => {
      box.classList.add("hidden");
      matches = [];
    };
    const render = () => {
      box.replaceChildren(
        ...matches.map((command, index) => {
          const item = document.createElement("li");
          item.setAttribute("role", "option");
          item.setAttribute("aria-selected", index === chosen ? "true" : "false");
          item.className = "cursor-pointer px-3 py-1.5 text-sm aria-selected:bg-haint-2 dark:aria-selected:bg-floor-2";
          const name = document.createElement("span");
          name.className = "font-mono font-semibold";
          name.textContent = `/${command.name}${command.usage ? ` ${command.usage}` : ""}`;
          const about = document.createElement("span");
          about.className = "ml-2 text-muted dark:text-haint";
          about.textContent = command.description;
          item.append(name, about);
          item.addEventListener("mousedown", (event) => {
            event.preventDefault();
            chosen = index;
            accept();
          });
          return item;
        }),
      );
      box.classList.toggle("hidden", matches.length === 0);
    };
    const accept = () => {
      const command = matches[chosen];
      if (!command) return;
      textarea.value = `/${command.name} `;
      textarea.dispatchEvent(new Event("input"));
      close();
    };
    textarea.addEventListener("input", async () => {
      const typed = /^\/([a-z0-9_-]*)$/i.exec(textarea.value);
      if (!typed) return close();
      commandList ||= fetch("/commands").then((response) => (response.ok ? response.json() : []));
      const all = await commandList.catch(() => []);
      matches = all.filter((command) => command.name.startsWith(typed[1].toLowerCase())).slice(0, 8);
      chosen = 0;
      render();
    });
    textarea.addEventListener("blur", () => setTimeout(close, 150));
    return {
      handleKey(event) {
        if (matches.length === 0) return false;
        if (event.key === "ArrowDown" || event.key === "ArrowUp") {
          event.preventDefault();
          chosen = (chosen + (event.key === "ArrowDown" ? 1 : matches.length - 1)) % matches.length;
          render();
          return true;
        }
        if (event.key === "Tab" || (event.key === "Enter" && !event.shiftKey)) {
          event.preventDefault();
          accept();
          return true;
        }
        if (event.key === "Escape") {
          close();
          return true;
        }
        return false;
      },
    };
  }

  // GIF search, shared by every composer on the page.
  const gifPicker = document.getElementById("gif-picker");
  let gifForm = null;
  let gifTimer = 0;
  // KLIPY's terms want searches and media loads to come from the browser,
  // so its Tenor-style API is called from here rather than the server.
  async function klipyGifs(query) {
    const { klipyKey: key, klipyFilter: filter } = gifPicker.dataset;
    const params = new URLSearchParams({ key, client_key: "sideporch", limit: "24", media_filter: "gif,tinygif", contentfilter: filter });
    if (query.trim()) params.set("q", query.trim());
    const response = await fetch(`https://api.klipy.com/v2/${query.trim() ? "search" : "featured"}?${params}`, { referrerPolicy: "no-referrer" });
    if (!response.ok) throw new Error(`KLIPY answered ${response.status}`);
    const { results = [] } = await response.json();
    return results.flatMap((result) => {
      const preview = result.media_formats?.tinygif ?? result.media_formats?.gif;
      const full = result.media_formats?.gif ?? preview;
      if (!preview?.url || !full?.url) return [];
      const [width, height] = full.dims ?? [0, 0];
      const [previewWidth, previewHeight] = preview.dims ?? [0, 0];
      return [{
        id: String(result.id),
        title: result.content_description || result.title || "GIF",
        preview: preview.url,
        width: previewWidth,
        height: previewHeight,
        send: { gif_url: full.url, gif_title: result.content_description || result.title || "GIF", gif_width: String(width), gif_height: String(height) },
      }];
    });
  }
  async function searchGifs() {
    const results = gifPicker.querySelector("[data-gif-results]");
    const status = gifPicker.querySelector("[data-gif-status]");
    const query = gifPicker.querySelector("[data-gif-search]").value;
    status.textContent = "Loading…";
    try {
      let gifs;
      if (gifPicker.dataset.provider === "klipy") {
        gifs = await klipyGifs(query);
      } else {
        const response = await fetch(`/gifs?q=${encodeURIComponent(query)}`);
        if (!response.ok) throw new Error(await response.text());
        ({ results: gifs } = await response.json());
      }
      results.replaceChildren(
        ...gifs.map((gif) => {
          const button = document.createElement("button");
          button.type = "button";
          button.className = "overflow-hidden rounded-lg bg-screen hover:ring-2 hover:ring-floor-3 dark:bg-night";
          button.title = gif.title;
          const image = document.createElement("img");
          image.src = gif.preview;
          image.alt = gif.title;
          image.loading = "lazy";
          image.referrerPolicy = "no-referrer";
          image.className = "h-auto w-full";
          if (gif.width && gif.height) {
            image.width = gif.width;
            image.height = gif.height;
          }
          button.append(image);
          button.addEventListener("click", () => sendGif(gif.id, gif.send));
          return button;
        }),
      );
      const empty = gifPicker.dataset.provider === "local" && !query ? "The library is empty. Add GIFs below." : "No GIFs found.";
      status.textContent = gifs.length ? "" : empty;
    } catch {
      status.textContent = "GIF search is unavailable right now.";
    }
  }
  function openGifs(form, trigger) {
    if (!gifPicker?.showPopover) return;
    gifForm = form;
    gifPicker.showPopover();
    const box = trigger.getBoundingClientRect();
    gifPicker.style.left = `${Math.max(8, Math.min(box.left, innerWidth - gifPicker.offsetWidth - 8))}px`;
    // Anchor the bottom edge, so the picker grows upward as results arrive.
    gifPicker.style.bottom = `${Math.max(8, innerHeight - box.top + 8)}px`;
    const search = gifPicker.querySelector("[data-gif-search]");
    search.focus();
    if (!gifPicker.dataset.loaded) {
      gifPicker.dataset.loaded = "1";
      search.addEventListener("input", () => {
        clearTimeout(gifTimer);
        gifTimer = setTimeout(searchGifs, 300);
      });
      searchGifs();
    }
  }
  async function sendGif(id, extra = {}) {
    if (!gifForm) return;
    gifPicker.hidePopover();
    const body = new URLSearchParams({ ...extra, gif: id });
    const parent = gifForm.elements.parent_id?.value;
    if (parent) body.set("parent_id", parent);
    const response = await fetch(gifForm.action, { method: "POST", headers: { "x-sideporch-fetch": "1" }, body });
    if (!response.ok) {
      const error = gifForm.querySelector("[data-composer-error]");
      error.textContent = "The GIF wasn't sent. Try again.";
      error.classList.remove("hidden");
    }
  }

  // Pages marked with data-refresh reload their content every few seconds.
  const refreshing = document.querySelector("[data-refresh]");
  if (refreshing) {
    setInterval(async () => {
      if (document.visibilityState !== "visible") return;
      const response = await fetch(location.href).catch(() => null);
      if (!response?.ok) return;
      const next = new DOMParser().parseFromString(await response.text(), "text/html").getElementById(refreshing.id);
      if (next) {
        refreshing.innerHTML = next.innerHTML;
        localizeTimes(refreshing);
      }
    }, Number(refreshing.dataset.refresh) * 1000);
  }

  function setupComposer(form) {
    const textarea = form.querySelector("textarea");
    const button = form.querySelector("button[type=submit]");
    const error = form.querySelector("[data-composer-error]");
    const fileInput = form.querySelector("input[type=file]");
    const fileList = form.querySelector("[data-file-list]");
    const draftKey = `sideporch:draft:${form.action}:${form.elements.parent_id?.value ?? ""}`;
    textarea.enterKeyHint = touch ? "enter" : "send";
    textarea.value = sessionStorage.getItem(draftKey) ?? "";

    const resize = () => {
      textarea.style.height = "auto";
      textarea.style.height = `${textarea.scrollHeight}px`;
    };
    resize();

    let typingSent = 0;
    textarea.addEventListener("input", () => {
      resize();
      sessionStorage.setItem(draftKey, textarea.value);
      const text = textarea.value.trim();
      const typing = text && !text.startsWith("/");
      const due = typing ? Date.now() - typingSent > 3000 : typingSent > 0;
      if (due && liveSocket?.readyState === WebSocket.OPEN) {
        typingSent = typing ? Date.now() : 0;
        const parent = form.elements.parent_id?.value;
        liveSocket.send(JSON.stringify({
          type: "typing",
          channel_id: Number(app.dataset.channel),
          parent_id: parent ? Number(parent) : null,
          stopped: !typing,
        }));
      }
    });
    const showFiles = () => {
      fileList.replaceChildren(
        ...[...fileInput.files].map((file) => {
          const item = document.createElement("li");
          item.className = "flex items-center gap-1.5 rounded-md bg-screen px-2 py-0.5 dark:bg-night-2";
          if (file.type.startsWith("image/")) {
            const preview = document.createElement("img");
            preview.src = URL.createObjectURL(file);
            preview.alt = "";
            preview.className = "h-8 w-8 rounded object-cover";
            item.append(preview);
          }
          item.append(file.name);
          return item;
        }),
      );
      fileList.classList.toggle("hidden", fileInput.files.length === 0);
      fileList.classList.toggle("flex", fileInput.files.length > 0);
    };
    fileInput?.addEventListener("change", showFiles);

    // Pasted or dropped files join the attachments.
    const addFiles = (files) => {
      if (!fileInput || files.length === 0) return false;
      const transfer = new DataTransfer();
      for (const file of [...fileInput.files, ...files]) transfer.items.add(file);
      fileInput.files = transfer.files;
      showFiles();
      return true;
    };
    textarea.addEventListener("paste", (event) => {
      const files = [...(event.clipboardData?.files ?? [])];
      if (addFiles(files)) event.preventDefault();
    });
    form.addEventListener("dragover", (event) => {
      if (event.dataTransfer?.types.includes("Files")) event.preventDefault();
    });
    form.addEventListener("drop", (event) => {
      const files = [...(event.dataTransfer?.files ?? [])];
      if (addFiles(files)) event.preventDefault();
    });
    form.querySelector("[data-gif-button]")?.addEventListener("click", (event) => openGifs(form, event.currentTarget));
    const scheduleButton = form.querySelector("[data-schedule-button]");
    if (scheduleButton) {
      scheduleButton.hidden = false;
      scheduleButton.addEventListener("click", (event) => openSchedule(form, event.currentTarget));
    }

    const suggestions = setupCommandSuggestions(form, textarea);
    textarea.addEventListener("keydown", (event) => {
      if (suggestions.handleKey(event)) return;
      // Up in an empty composer edits your last message here.
      if (event.key === "ArrowUp" && !textarea.value && !event.shiftKey && !event.altKey && !event.metaKey && !event.ctrlKey) {
        const list = document.getElementById(form.elements.parent_id ? "replies" : "messages");
        const mine = [...(list?.querySelectorAll(`li[data-user="${app?.dataset.me}"]:not([data-deleted])`) ?? [])].pop();
        if (mine && stillEditable(mine)) {
          event.preventDefault();
          startEdit(mine);
          return;
        }
      }
      // On phones, Enter starts a new line and the Send button sends.
      if (event.key === "Enter" && !event.shiftKey && !event.isComposing && !touch) {
        event.preventDefault();
        form.requestSubmit();
      }
    });

    form.addEventListener("submit", async (event) => {
      event.preventDefault();
      const hasFiles = fileInput && fileInput.files.length > 0;
      if ((!textarea.value.trim() && !hasFiles) || button.disabled) return;
      button.disabled = true;
      error.classList.add("hidden");
      try {
        const response = await fetch(form.action, {
          method: "POST",
          headers: { "x-sideporch-fetch": "1" },
          body: hasFiles ? new FormData(form) : new URLSearchParams(new FormData(form)),
        });
        if (!response.ok) {
          const reason = response.status === 413 ? "Those files are too large." : await response.text();
          throw new Error(reason.includes("<") ? "" : reason);
        }
        // A slash command answers privately instead of posting.
        if (response.headers.get("content-type")?.includes("application/json")) {
          const { ephemeral = [] } = await response.json();
          const thread = form.elements.parent_id?.value;
          const list = document.getElementById(thread ? "replies" : "messages");
          const scroller = document.getElementById(thread ? "thread-scroller" : "scroller");
          for (const html of ephemeral) {
            const template = document.createElement("template");
            template.innerHTML = html.trim();
            if (list && template.content.firstElementChild) list.append(template.content.firstElementChild);
          }
          scrollToEnd(scroller);
        }
        textarea.value = "";
        if (fileInput) fileInput.value = "";
        if (fileList) showFiles();
        sessionStorage.removeItem(draftKey);
        resize();
      } catch (failure) {
        error.textContent = failure.message || "Your message wasn't sent. Check your connection and try again.";
        error.classList.remove("hidden");
      } finally {
        button.disabled = false;
        textarea.focus();
      }
    });
  }

  async function sendReaction(channelId, messageId, emoji) {
    await fetch(`/c/${channelId}/m/${messageId}/reactions`, {
      method: "POST",
      headers: { "x-sideporch-fetch": "1" },
      body: new URLSearchParams({ emoji }),
    });
  }

  // Poll votes and automation buttons post in the background; the live
  // update redraws the message.
  document.addEventListener("submit", (event) => {
    const form = event.target.closest("form[data-background]");
    if (!form) return;
    event.preventDefault();
    fetch(form.action, { method: "POST", headers: { "x-sideporch-fetch": "1" }, body: new URLSearchParams(new FormData(form)) })
      .then((response) => {
        if (!response.ok) toast("That didn't work. Try again.");
      })
      .catch(() => toast("That didn't work. Check your connection."));
  });

  // Reaction chips toggle in the background; the live update redraws them.
  function setupReactions() {
    document.addEventListener("submit", (event) => {
      const form = event.target.closest("form[data-reaction-form]");
      if (!form) return;
      event.preventDefault();
      const emoji = event.submitter?.value;
      const [, channelId, messageId] = form.action.match(/\/c\/(\d+)\/m\/(\d+)\//) ?? [];
      if (emoji && channelId) sendReaction(channelId, messageId, emoji);
    });

    const picker = document.getElementById("emoji-picker");
    if (!picker || !picker.showPopover) return;
    const search = picker.querySelector("[data-emoji-search]");
    const scroll = picker.querySelector("[data-emoji-scroll]");
    const empty = picker.querySelector("[data-emoji-empty]");
    let catalog = null;

    // All standard emoji, loaded once, the first time the picker opens.
    const loadCatalog = () => {
      catalog ||= fetch(`/assets/emoji.json?v=${document.documentElement.dataset.assets || ""}`)
        .then((response) => response.json())
        .then(({ categories }) => {
          const holder = picker.querySelector("[data-emoji-catalog]");
          const tabs = picker.querySelector("[data-emoji-tabs]");
          for (const [index, category] of categories.entries()) {
            const section = document.createElement("section");
            section.dataset.pickerSection = "";
            section.id = `emoji-category-${index}`;
            const heading = document.createElement("h3");
            heading.className = "sticky top-0 z-[1] bg-white px-1 py-1 text-xs font-semibold text-muted dark:bg-night-2 dark:text-haint";
            heading.textContent = category.name;
            const grid = document.createElement("div");
            grid.className = "grid grid-cols-8 gap-0.5";
            for (const [name, glyph, keywords] of category.emoji) {
              const button = document.createElement("button");
              button.type = "button";
              button.dataset.emoji = name;
              button.dataset.keywords = `${name} ${keywords}`.toLowerCase();
              button.title = `:${name}:`;
              button.setAttribute("aria-label", name.replace(/_/g, " "));
              button.className = "flex h-8 items-center justify-center rounded-md text-xl hover:bg-screen dark:hover:bg-night";
              button.textContent = glyph;
              grid.append(button);
            }
            section.append(heading, grid);
            holder.append(section);
            const tab = document.createElement("button");
            tab.type = "button";
            tab.title = category.name;
            tab.setAttribute("aria-label", category.name);
            tab.className = "rounded-md px-1 hover:bg-screen dark:hover:bg-night";
            tab.textContent = category.emoji[0]?.[1] ?? "•";
            tab.addEventListener("click", () => {
              search.value = "";
              filter();
              section.scrollIntoView({ block: "start" });
            });
            tabs.append(tab);
          }
        })
        .catch((error) => console.warn("sideporch: emoji unavailable", error));
      return catalog;
    };

    const filter = () => {
      const query = search.value.trim().toLowerCase().replace(/^:|:$/g, "");
      let shown = 0;
      for (const section of picker.querySelectorAll("[data-picker-section]")) {
        let visible = 0;
        for (const button of section.querySelectorAll("[data-emoji]")) {
          const match = !query || (button.dataset.keywords || button.dataset.emoji).includes(query);
          button.hidden = !match;
          if (match) visible += 1;
        }
        section.hidden = visible === 0;
        shown += visible;
      }
      empty.hidden = shown > 0;
    };
    search.addEventListener("input", filter);
    search.addEventListener("keydown", (event) => {
      if (event.key !== "Enter") return;
      event.preventDefault();
      const first = [...picker.querySelectorAll("[data-emoji]")].find((button) => !button.hidden && !button.closest("[hidden]"));
      first?.click();
    });

    document.addEventListener("click", (event) => {
      const trigger = event.target.closest("a[data-react]");
      if (!trigger) return;
      event.preventDefault();
      picker.dataset.message = trigger.dataset.react;
      search.value = "";
      filter();
      scroll.scrollTop = 0;
      picker.showPopover();
      loadCatalog().then(filter);
      search.focus();
      const box = trigger.getBoundingClientRect();
      const top = Math.min(box.bottom + 6, innerHeight - picker.offsetHeight - 8);
      const left = Math.max(8, Math.min(box.right - picker.offsetWidth, innerWidth - picker.offsetWidth - 8));
      picker.style.top = `${Math.max(8, top)}px`;
      picker.style.left = `${left}px`;
    });
    picker.addEventListener("click", (event) => {
      const choice = event.target.closest("[data-emoji]");
      if (!choice) return;
      picker.hidePopover();
      sendReaction(app.dataset.channel, picker.dataset.message, choice.dataset.emoji);
    });
  }

  // A message's menu: edit, pin, save, copy a link and delete, in place.
  // Without JavaScript the same link opens a page with these actions.
  const messageMenu = document.createElement("div");
  messageMenu.id = "message-menu";
  messageMenu.setAttribute("popover", "");
  messageMenu.className = "m-0 w-56 rounded-xl border border-line bg-white py-1 text-sm shadow-xl dark:border-night-line dark:bg-night-2 dark:text-haint-2";
  document.body.append(messageMenu);

  const post = (url, fields = {}) =>
    fetch(url, { method: "POST", headers: { "x-sideporch-fetch": "1" }, body: new URLSearchParams(fields) });

  function openMessageMenu(trigger, extraEntries = []) {
    const item = trigger.closest("li[data-message-id]");
    if (!item || !messageMenu.showPopover) return false;
    const base = `/c/${item.dataset.channelId}/m/${item.dataset.messageId}`;
    const mine = item.dataset.user === app?.dataset.me;
    const moderator = app?.dataset.moderator !== undefined;
    const deleted = item.dataset.deleted !== undefined;
    const entries = [];
    if (mine && !deleted && stillEditable(item)) entries.push(["Edit message", () => startEdit(item)]);
    if (!deleted) {
      entries.push([item.dataset.pinned !== undefined ? "Unpin" : "Pin to channel", () => post(`${base}/pin`)]);
      entries.push([
        item.dataset.saved !== undefined ? "Remove from saved" : "Save for later",
        async () => {
          const response = await post(`${base}/save`);
          if (!response.ok) return;
          const { saved } = await response.json();
          for (const copy of messageCopies(item.dataset.messageId)) {
            if (saved) copy.dataset.saved = "";
            else delete copy.dataset.saved;
          }
        },
      ]);
    }
    entries.push(...extraEntries.map((entry) => entry(item, base)).filter(Boolean));
    entries.push(["Copy link", () => navigator.clipboard?.writeText(`${location.origin}${base}`)]);
    if (!mine && !deleted && item.dataset.user) {
      entries.push([
        "Report message",
        async () => {
          const reason = prompt("What's wrong with this message? Moderators will look at it.");
          if (reason === null) return;
          const response = await post(`${base}/report`, { reason });
          toast(response.ok ? "Thanks. Moderators will look at it." : "That report didn't go through. Try again.");
        },
      ]);
    }
    if ((mine || moderator) && !deleted) {
      entries.push([
        "Delete message",
        async () => {
          if (confirm("Delete this message? This can't be undone.")) await post(`${base}/delete`);
        },
        "text-red-700 dark:text-red-300",
      ]);
    }
    messageMenu.replaceChildren(
      ...entries.map(([label, run, tone = ""]) => {
        const button = document.createElement("button");
        button.type = "button";
        button.className = `block w-full px-3 py-1.5 text-left hover:bg-screen dark:hover:bg-night ${tone}`;
        button.textContent = label;
        button.addEventListener("click", () => {
          messageMenu.hidePopover();
          run();
        });
        return button;
      }),
    );
    messageMenu.showPopover();
    const box = trigger.getBoundingClientRect();
    messageMenu.style.top = `${Math.max(8, Math.min(box.bottom + 4, innerHeight - messageMenu.offsetHeight - 8))}px`;
    messageMenu.style.left = `${Math.max(8, Math.min(box.right - messageMenu.offsetWidth, innerWidth - messageMenu.offsetWidth - 8))}px`;
    messageMenu.querySelector("button")?.focus();
    return true;
  }
  function toast(text) {
    const note = document.createElement("p");
    note.setAttribute("role", "status");
    note.className = "fixed bottom-6 left-1/2 z-50 -translate-x-1/2 rounded-xl bg-floor px-4 py-2 text-sm text-white shadow-xl";
    note.textContent = text;
    document.body.append(note);
    setTimeout(() => note.remove(), 4000);
  }
  async function remindAbout(base, when) {
    const response = await post(`${base}/remind`, { when });
    if (!response.ok) return toast("That reminder didn't work. Try again.");
    const { at } = await response.json();
    toast(`I'll remind you ${at}, in your notes to self.`);
  }
  // Reading aloud: the server's voice when it has one, else the device's.
  let reading = null;
  function stopReading() {
    reading?.audio?.pause();
    if (reading?.device) speechSynthesis.cancel();
    reading = null;
  }
  function readOnDevice(item) {
    if (!("speechSynthesis" in window)) return toast("This browser can't read aloud.");
    const text = item.querySelector("[data-body]")?.innerText?.trim();
    if (!text) return;
    const utterance = new SpeechSynthesisUtterance(text);
    utterance.onend = () => (reading = null);
    reading = { id: item.dataset.messageId, device: true };
    speechSynthesis.speak(utterance);
  }
  function readAloud(item, base) {
    stopReading();
    if (app?.dataset.voice !== "server") return readOnDevice(item);
    const audio = new Audio(`${base}/speech`);
    reading = { id: item.dataset.messageId, audio };
    audio.onended = () => (reading = null);
    audio.onerror = () => {
      toast("The server's voice didn't work; this device reads it instead.");
      readOnDevice(item);
    };
    toast("Reading aloud…");
    audio.play().catch(() => {});
  }
  const menuExtras = [
    (item, base) => {
      if (item.dataset.deleted !== undefined || !item.querySelector("[data-body]")) return null;
      return reading?.id === item.dataset.messageId ? ["Stop reading", stopReading] : ["Read aloud", () => readAloud(item, base)];
    },
    (item, base) => (item.dataset.deleted === undefined ? ["Remind me in 1 hour", () => remindAbout(base, "in 1 hour")] : null),
    (item, base) => (item.dataset.deleted === undefined ? ["Remind me tomorrow", () => remindAbout(base, "tomorrow")] : null),
    (item, base) =>
      item.querySelector("[data-preview]") && (item.dataset.user === app?.dataset.me || app?.dataset.admin !== undefined)
        ? ["Remove preview", () => post(`${base}/preview/remove`)]
        : null,
  ];

  // Send later: presets and a date and time, from the clock next to Send.
  const schedulePicker = document.createElement("div");
  schedulePicker.id = "schedule-picker";
  schedulePicker.setAttribute("popover", "");
  schedulePicker.className = "m-0 w-64 rounded-xl border border-line bg-white p-2 text-sm shadow-xl dark:border-night-line dark:bg-night-2 dark:text-haint-2";
  document.body.append(schedulePicker);
  function openSchedule(form, trigger) {
    if (!schedulePicker.showPopover) return;
    const textarea = form.querySelector("textarea");
    const error = form.querySelector("[data-composer-error]");
    const send = (when) => {
      schedulePicker.hidePopover();
      if (!textarea.value.trim()) {
        error.textContent = "Write the message first, then pick when to send it.";
        error.classList.remove("hidden");
        return;
      }
      const field = document.createElement("input");
      field.type = "hidden";
      field.name = "send_at";
      field.value = when;
      form.append(field);
      form.requestSubmit();
      field.remove();
    };
    const heading = document.createElement("p");
    heading.className = "px-2 pb-1 pt-1 text-xs font-semibold text-muted dark:text-haint";
    heading.textContent = "Send later";
    const presets = [
      ["In 1 hour", "in 1 hour"],
      ["Tomorrow at 9:00", "tomorrow at 9:00"],
      ["Monday at 9:00", "monday at 9:00"],
    ].map(([label, when]) => {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "block w-full rounded-lg px-2 py-1.5 text-left hover:bg-screen dark:hover:bg-night";
      button.textContent = label;
      button.addEventListener("click", () => send(when));
      return button;
    });
    const custom = document.createElement("form");
    custom.className = "mt-1 flex gap-1 border-t border-line px-1 pt-2 dark:border-night-line";
    const input = document.createElement("input");
    input.type = "datetime-local";
    input.required = true;
    input.className = "field min-w-0 flex-1 py-1 text-sm";
    input.setAttribute("aria-label", "Date and time");
    const pick = document.createElement("button");
    pick.type = "submit";
    pick.className = "btn px-2 py-1 text-xs";
    pick.textContent = "Schedule";
    custom.append(input, pick);
    custom.addEventListener("submit", (event) => {
      event.preventDefault();
      send(input.value);
    });
    schedulePicker.replaceChildren(heading, ...presets, custom);
    schedulePicker.showPopover();
    const box = trigger.getBoundingClientRect();
    schedulePicker.style.left = `${Math.max(8, Math.min(box.right - schedulePicker.offsetWidth, innerWidth - schedulePicker.offsetWidth - 8))}px`;
    schedulePicker.style.bottom = `${Math.max(8, innerHeight - box.top + 8)}px`;
  }
  document.addEventListener("click", (event) => {
    const trigger = event.target.closest("a[data-actions]");
    if (trigger && openMessageMenu(trigger, menuExtras)) event.preventDefault();
  });

  // Admins can limit how long messages stay editable.
  function stillEditable(item) {
    const minutes = Number(document.querySelector("[data-edit-minutes]")?.dataset.editMinutes || 0);
    return !minutes || Date.now() - Number(item.dataset.created) <= minutes * 60000;
  }

  // Edits a message in place: Enter saves, Escape cancels.
  async function startEdit(item) {
    if (item.querySelector("form[data-edit]")) return;
    const base = `/c/${item.dataset.channelId}/m/${item.dataset.messageId}`;
    const response = await fetch(`${base}/source`);
    if (!response.ok) return;
    const { body } = await response.json();
    const content = item.querySelector("[data-body]");
    const form = document.createElement("form");
    form.dataset.edit = "";
    form.className = "my-1";
    const textarea = document.createElement("textarea");
    textarea.className = "field min-h-20 text-sm";
    textarea.value = body;
    textarea.setAttribute("aria-label", "Edit message");
    const row = document.createElement("div");
    row.className = "mt-1 flex items-center gap-2 text-xs text-muted dark:text-haint";
    const hint = document.createElement("span");
    hint.textContent = "Enter to save, Escape to cancel";
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.className = "btn-quiet ml-auto px-2 py-1 text-xs";
    cancel.textContent = "Cancel";
    const save = document.createElement("button");
    save.type = "submit";
    save.className = "btn px-2 py-1 text-xs";
    save.textContent = "Save";
    row.append(hint, cancel, save);
    form.append(textarea, row);
    const close = () => {
      form.remove();
      if (content) content.hidden = false;
    };
    if (content) {
      content.hidden = true;
      content.after(form);
    } else {
      item.querySelector(".min-w-0.flex-1")?.append(form);
    }
    textarea.focus();
    textarea.setSelectionRange(textarea.value.length, textarea.value.length);
    cancel.addEventListener("click", close);
    textarea.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        event.preventDefault();
        close();
      } else if (event.key === "Enter" && !event.shiftKey && !event.isComposing && !touch) {
        event.preventDefault();
        form.requestSubmit();
      }
    });
    form.addEventListener("submit", async (event) => {
      event.preventDefault();
      if (!textarea.value.trim()) return;
      const answer = await post(`${base}/edit`, { body: textarea.value });
      if (answer.ok) {
        close();
      } else {
        hint.textContent = (await answer.text()).slice(0, 200) || "Couldn't save. Try again.";
        hint.className = "text-red-700 dark:text-red-300";
      }
    });
  }

  // Trying on themes: the page takes a theme or mode as soon as it is picked.
  const themePicker = document.querySelector("[data-theme-picker]");
  if (themePicker) {
    const root = document.documentElement;
    const systemDark = matchMedia("(prefers-color-scheme: dark)");
    themePicker.addEventListener("change", () => {
      const theme = themePicker.querySelector("input[name=theme]:checked");
      const mode = themePicker.querySelector("input[name=appearance]:checked")?.value || root.dataset.appearance;
      if (theme?.value) root.dataset.theme = theme.value;
      const hasLight = theme?.dataset.light !== "false";
      const hasDark = theme?.dataset.dark !== "false";
      const dark = !hasLight || (hasDark && (mode === "dark" || (mode === "system" && systemDark.matches)));
      root.classList.toggle("dark", dark);
    });
  }

  // Keyboard shortcuts: Cmd/Ctrl+K jumps anywhere, Alt+Up/Down moves
  // between channels (with Shift, unread ones), Escape closes a thread,
  // and Cmd/Ctrl+/ lists them all.
  function popoverPanel(id, className) {
    const panel = document.createElement("div");
    panel.id = id;
    panel.setAttribute("popover", "");
    panel.className = className;
    document.body.append(panel);
    return panel;
  }
  const switcher = popoverPanel(
    "switcher",
    "mx-auto mt-[12vh] w-[32rem] max-w-[92vw] overflow-hidden rounded-xl border border-line bg-white shadow-2xl dark:border-night-line dark:bg-night-2 dark:text-haint-2",
  );
  function openSwitcher() {
    if (!switcher.showPopover) return;
    const places = [...document.querySelectorAll("nav a[data-channel-link], nav a[data-nav-link]")].map((link) => ({
      label: link.textContent.trim(),
      href: link.getAttribute("href"),
      unread: link.dataset.unread !== undefined,
    }));
    places.push({ label: "Browse channels", href: "/channels/browse" }, { label: "People", href: "/people" });
    const input = document.createElement("input");
    input.type = "search";
    input.placeholder = "Jump to a channel, person or page";
    input.setAttribute("aria-label", input.placeholder);
    input.className = "w-full border-b border-line bg-transparent px-4 py-3 outline-hidden dark:border-night-line";
    const list = document.createElement("ul");
    list.className = "max-h-80 overflow-y-auto py-1";
    list.setAttribute("role", "listbox");
    let matches = [];
    let chosen = 0;
    const render = () => {
      const query = input.value.trim().toLowerCase();
      matches = places.filter((place) => place.label.toLowerCase().includes(query)).slice(0, 12);
      if (query) matches.push({ label: `Search messages for “${input.value.trim()}”`, href: `/search?q=${encodeURIComponent(input.value.trim())}` });
      chosen = Math.min(chosen, Math.max(0, matches.length - 1));
      list.replaceChildren(
        ...matches.map((place, index) => {
          const item = document.createElement("li");
          item.setAttribute("role", "option");
          item.setAttribute("aria-selected", index === chosen ? "true" : "false");
          item.className = "cursor-pointer px-4 py-1.5 aria-selected:bg-haint-2 dark:aria-selected:bg-floor-2";
          item.textContent = place.label;
          if (place.unread) item.classList.add("font-bold");
          item.addEventListener("mousedown", (event) => {
            event.preventDefault();
            location.href = place.href;
          });
          return item;
        }),
      );
    };
    input.addEventListener("input", () => {
      chosen = 0;
      render();
    });
    input.addEventListener("keydown", (event) => {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        chosen = (chosen + (event.key === "ArrowDown" ? 1 : matches.length - 1)) % Math.max(1, matches.length);
        render();
      } else if (event.key === "Enter" && matches[chosen]) {
        event.preventDefault();
        location.href = matches[chosen].href;
      }
    });
    switcher.replaceChildren(input, list);
    render();
    switcher.showPopover();
    input.focus();
  }

  const shortcutHelp = popoverPanel(
    "shortcuts",
    "mx-auto mt-[12vh] w-[28rem] max-w-[92vw] rounded-xl border border-line bg-white p-5 shadow-2xl dark:border-night-line dark:bg-night-2 dark:text-haint-2",
  );
  function openShortcuts() {
    if (!shortcutHelp.showPopover) return;
    const mod = /Mac|iPhone|iPad/.test(navigator.platform) ? "⌘" : "Ctrl";
    const rows = [
      [`${mod} K`, "Jump to a channel, person or page"],
      ["Alt ↑ / ↓", "Previous or next channel"],
      ["Alt Shift ↑ / ↓", "Previous or next unread channel"],
      ["↑", "Edit your last message (in an empty composer)"],
      ["Enter / Shift Enter", "Send / new line"],
      ["Escape", "Close the thread"],
      [`${mod} /`, "Show these shortcuts"],
    ];
    const heading = document.createElement("h2");
    heading.className = "mb-3 text-lg font-bold";
    heading.textContent = "Keyboard shortcuts";
    const table = document.createElement("dl");
    table.className = "grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm";
    for (const [keys, what] of rows) {
      const term = document.createElement("dt");
      term.className = "font-mono font-semibold";
      term.textContent = keys;
      const description = document.createElement("dd");
      description.textContent = what;
      table.append(term, description);
    }
    shortcutHelp.replaceChildren(heading, table);
    shortcutHelp.showPopover();
  }

  function moveChannel(step, unreadOnly) {
    const links = [...document.querySelectorAll("nav a[data-channel-link]")];
    if (links.length === 0) return;
    const current = links.findIndex((link) => link.getAttribute("aria-current") === "page");
    for (let offset = 1; offset <= links.length; offset += 1) {
      const index = (current + step * offset + links.length * offset) % links.length;
      const link = links[index];
      if (!unreadOnly || link.dataset.unread !== undefined) {
        location.href = link.getAttribute("href");
        return;
      }
    }
  }

  document.addEventListener("keydown", (event) => {
    const mod = event.metaKey || event.ctrlKey;
    const key = event.key.toLowerCase();
    if (mod && key === "k") {
      event.preventDefault();
      openSwitcher();
    } else if (mod && key === "/") {
      event.preventDefault();
      openShortcuts();
    } else if (event.altKey && (event.key === "ArrowUp" || event.key === "ArrowDown")) {
      event.preventDefault();
      moveChannel(event.key === "ArrowUp" ? -1 : 1, event.shiftKey);
    } else if (event.key === "Escape" && app?.dataset.thread && !document.querySelector(":popover-open") && !event.target.closest?.("form[data-edit]")) {
      const textarea = event.target.closest?.("form[data-composer]")?.querySelector("textarea");
      if (!textarea?.value) location.href = `/c/${app.dataset.channel}`;
    }
  });

  // Nudges people towards a good phone setup: on iPhone and iPad, adding
  // Sideporch to the home screen (the only way to get notifications there);
  // elsewhere, installing it or turning notifications on. Each hint can be
  // dismissed for good on this device.
  function setupInstallHint() {
    const card = document.querySelector("[data-install]");
    if (!card) return;
    const text = card.querySelector("[data-install-text]");
    const action = card.querySelector("[data-install-action]");
    const standalone = matchMedia("(display-mode: standalone)").matches || navigator.standalone === true;
    const iPad = navigator.platform === "MacIntel" && navigator.maxTouchPoints > 1;
    const iOS = /iPhone|iPad|iPod/.test(navigator.userAgent) || iPad;
    const show = (key, message, label, run) => {
      if (localStorage.getItem(`sideporch:hint:${key}`)) return;
      text.textContent = message;
      action.hidden = !label;
      action.textContent = label || "";
      action.onclick = () => {
        card.hidden = true;
        run();
      };
      card.querySelector("[data-install-dismiss]").onclick = () => {
        localStorage.setItem(`sideporch:hint:${key}`, "1");
        card.hidden = true;
      };
      card.hidden = false;
    };
    if (iOS && !standalone) {
      const device = /iPad/.test(navigator.userAgent) || iPad ? "iPad" : "iPhone";
      show("ios", `To get notifications on this ${device}, tap Share, then Add to Home Screen, and open Sideporch from there.`);
      return;
    }
    if ("PushManager" in window && window.Notification?.permission === "default" && (standalone || matchMedia("(pointer: coarse)").matches)) {
      show("notifications", "Turn on notifications to hear about direct messages, mentions and replies.", "Turn on", () =>
        document.querySelector("[data-push-toggle]")?.click(),
      );
    }
    addEventListener("beforeinstallprompt", (event) => {
      event.preventDefault();
      show("install", "Install Sideporch to open it like an app, in its own window.", "Install", async () => {
        event.prompt();
        await event.userChoice.catch(() => null);
      });
    });
  }

  // The app icon shows how many conversations are unread, and opening one
  // clears its notifications.
  function updateAppBadge() {
    if (!("setAppBadge" in navigator)) return;
    const unread = document.querySelectorAll("nav a[data-channel-link][data-unread]").length;
    (unread > 0 ? navigator.setAppBadge(unread) : navigator.clearAppBadge()).catch(() => {});
  }
  async function clearNotifications() {
    if (!app?.dataset.channel || document.visibilityState !== "visible" || !navigator.serviceWorker) return;
    const registration = await navigator.serviceWorker.getRegistration();
    const shown = (await registration?.getNotifications({ tag: `channel-${app.dataset.channel}` })) ?? [];
    for (const notification of shown) notification.close();
  }

  function base64UrlToBytes(text) {
    const base64 = (text + "===".slice((text.length + 3) % 4)).replace(/-/g, "+").replace(/_/g, "/");
    return Uint8Array.from(atob(base64), (char) => char.charCodeAt(0));
  }

  // The bell in the sidebar turns push notifications on and off for this
  // browser. On iPhone and iPad this needs Sideporch added to the home screen.
  async function setupPush() {
    if (!("serviceWorker" in navigator)) return;
    // The worker also shows the offline page, so register it everywhere.
    const registration = await navigator.serviceWorker.register("/sw.js");
    const toggle = document.querySelector("[data-push-toggle]");
    if (!toggle || !("PushManager" in window) || !window.Notification) return;
    const show = (subscribed) => {
      toggle.setAttribute("aria-pressed", subscribed ? "true" : "false");
      const state = toggle.querySelector("[data-push-state]");
      if (state) state.textContent = subscribed ? "On" : "Off";
    };
    show(Boolean(await registration.pushManager.getSubscription()));
    toggle.hidden = false;
    toggle.addEventListener("click", async () => {
      const post = (url, subscription) =>
        fetch(url, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(subscription) });
      const existing = await registration.pushManager.getSubscription();
      if (existing) {
        await post("/push/unsubscribe", existing.toJSON());
        await existing.unsubscribe();
        show(false);
        return;
      }
      if ((await Notification.requestPermission()) !== "granted") return;
      const key = await (await fetch("/push/key")).text();
      const subscription = await registration.pushManager.subscribe({
        userVisibleOnly: true,
        applicationServerKey: base64UrlToBytes(key),
      });
      const response = await post("/push/subscriptions", subscription.toJSON());
      show(response.ok);
    });
  }

  // The account menu closes on a click elsewhere or Escape.
  const accountMenu = document.querySelector("[data-account-menu]");
  if (accountMenu) {
    document.addEventListener("click", (event) => {
      if (accountMenu.open && !accountMenu.contains(event.target)) accountMenu.open = false;
    });
    accountMenu.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && accountMenu.open) {
        accountMenu.open = false;
        accountMenu.querySelector("summary")?.focus();
      }
    });
  }

  function setupCopyButtons() {
    for (const button of document.querySelectorAll("[data-copy]")) {
      button.addEventListener("click", async () => {
        const label = button.querySelector("[data-copy-label]");
        try {
          await navigator.clipboard.writeText(button.dataset.copy);
          if (label) label.textContent = "Copied";
        } catch {
          if (label) label.textContent = "Select and copy";
        }
        setTimeout(() => label && (label.textContent = "Copy"), 2000);
      });
    }
  }

  // Matching channels, people and messages under the sidebar's search box,
  // while typing. Enter without a choice opens the full results.
  function setupQuickSearch() {
    const form = document.querySelector("form[data-quick-search]");
    const input = form?.querySelector("input[name=q]");
    const list = form?.querySelector("#quick-results");
    if (!input || !list) return;
    let timer = 0;
    let active = -1;
    let latest = 0;
    const items = () => [...list.querySelectorAll("[role=option]")];
    const close = () => {
      list.hidden = true;
      input.setAttribute("aria-expanded", "false");
      active = -1;
    };
    const highlight = (index) => {
      const options = items();
      active = Math.max(-1, Math.min(index, options.length - 1));
      options.forEach((option, i) => option.setAttribute("aria-selected", i === active ? "true" : "false"));
      options[active]?.scrollIntoView({ block: "nearest" });
    };
    const kinds = { channel: "Channel", person: "Person", message: "Message" };
    const show = (data) => {
      list.replaceChildren();
      if (data.corrected) {
        const note = document.createElement("li");
        note.className = "px-3 py-1 text-xs text-muted";
        note.textContent = `Showing results for “${data.corrected}”`;
        list.append(note);
      }
      for (const item of data.items) {
        const option = document.createElement("li");
        option.setAttribute("role", "option");
        option.setAttribute("aria-selected", "false");
        const link = document.createElement("a");
        link.href = item.href;
        link.tabIndex = -1;
        link.className = "block rounded-lg px-3 py-1.5 hover:bg-screen dark:hover:bg-night";
        const label = document.createElement("span");
        label.className = "block truncate text-sm font-semibold";
        label.textContent = item.label;
        const detail = document.createElement("span");
        detail.className = "block truncate text-xs text-muted";
        detail.textContent = [kinds[item.kind], item.detail].filter(Boolean).join(" · ");
        link.append(label, detail);
        option.append(link);
        list.append(option);
      }
      const all = document.createElement("li");
      all.setAttribute("role", "option");
      all.setAttribute("aria-selected", "false");
      const link = document.createElement("a");
      link.href = `/search?q=${encodeURIComponent(input.value)}`;
      link.tabIndex = -1;
      link.className = "block rounded-lg px-3 py-1.5 text-sm font-semibold text-floor-3 hover:bg-screen dark:text-haint dark:hover:bg-night";
      link.textContent = data.items.length ? "All results" : "Search everything";
      all.append(link);
      list.append(all);
      list.hidden = false;
      input.setAttribute("aria-expanded", "true");
      active = -1;
    };
    input.addEventListener("input", () => {
      clearTimeout(timer);
      const text = input.value.trim();
      if (text.length < 2) return close();
      timer = setTimeout(async () => {
        const request = ++latest;
        const response = await fetch(`/search/suggest?q=${encodeURIComponent(text)}`).catch(() => null);
        if (!response?.ok || request !== latest) return;
        show(await response.json());
      }, 150);
    });
    input.addEventListener("keydown", (event) => {
      if (list.hidden) return;
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        highlight(active + (event.key === "ArrowDown" ? 1 : -1));
      } else if (event.key === "Enter" && active >= 0) {
        event.preventDefault();
        items()[active]?.querySelector("a")?.click();
      } else if (event.key === "Escape") {
        close();
      }
    });
    input.addEventListener("blur", () => setTimeout(close, 150));
  }

  // Passkeys. The server speaks base64url; WebAuthn speaks ArrayBuffers.
  const fromBase64 = (text) => {
    const padded = text.replace(/-/g, "+").replace(/_/g, "/") + "=".repeat((4 - (text.length % 4)) % 4);
    return Uint8Array.from(atob(padded), (c) => c.charCodeAt(0));
  };
  const toBase64 = (buffer) =>
    btoa(String.fromCharCode(...new Uint8Array(buffer)))
      .replace(/\+/g, "-")
      .replace(/\//g, "_")
      .replace(/=+$/, "");
  const postJson = (url, body) =>
    fetch(url, { method: "POST", headers: { "content-type": "application/json", "x-sideporch-fetch": "1" }, body: JSON.stringify(body) });
  // Error pages carry their message in the first paragraph.
  const errorText = async (response) => {
    const page = await response.text();
    return new DOMParser().parseFromString(page, "text/html").querySelector("main p, p")?.textContent || "That didn't work. Try again.";
  };
  const showPasskeyError = (text) => {
    const error = document.querySelector("[data-passkey-error]");
    if (!error) return toast(text);
    error.textContent = text;
    error.classList.remove("hidden");
  };
  // Cancelling the browser's prompt isn't an error worth showing.
  const passkeyFailed = (error) => {
    if (error?.name === "AbortError" || error?.name === "NotAllowedError") return;
    showPasskeyError(error?.message || "The passkey didn't work. Try again.");
  };

  async function addPasskey() {
    try {
      const started = await postJson("/webauthn/register/options", {});
      if (!started.ok) throw new Error(await errorText(started));
      const options = await started.json();
      const publicKey = options.publicKey;
      publicKey.challenge = fromBase64(publicKey.challenge);
      publicKey.user.id = fromBase64(publicKey.user.id);
      publicKey.excludeCredentials = publicKey.excludeCredentials.map((known) => ({ ...known, id: fromBase64(known.id) }));
      const credential = await navigator.credentials.create({ publicKey });
      const response = credential.response;
      const name = prompt("Name this passkey, so you recognize it later (like “Work laptop”):", "") ?? "";
      const result = await postJson("/webauthn/register", {
        ceremony: options.ceremony,
        id: toBase64(credential.rawId),
        clientDataJSON: toBase64(response.clientDataJSON),
        authenticatorData: toBase64(response.getAuthenticatorData()),
        publicKey: toBase64(response.getPublicKey()),
        publicKeyAlgorithm: response.getPublicKeyAlgorithm(),
        transports: response.getTransports?.() ?? [],
        name,
      });
      if (!result.ok) throw new Error(await errorText(result));
      location.href = (await result.json()).redirect;
    } catch (error) {
      passkeyFailed(error);
    }
  }

  let passkeyWaiting = null;
  async function signInWithPasskey(next, mediation) {
    passkeyWaiting?.abort();
    const controller = new AbortController();
    passkeyWaiting = controller;
    try {
      const started = await postJson("/webauthn/login/options", {});
      if (!started.ok) throw new Error(await errorText(started));
      const options = await started.json();
      const publicKey = options.publicKey;
      publicKey.challenge = fromBase64(publicKey.challenge);
      publicKey.allowCredentials = publicKey.allowCredentials.map((known) => ({ ...known, id: fromBase64(known.id) }));
      const credential = await navigator.credentials.get({ publicKey, mediation, signal: controller.signal });
      const response = credential.response;
      const result = await postJson("/webauthn/login", {
        ceremony: options.ceremony,
        id: toBase64(credential.rawId),
        clientDataJSON: toBase64(response.clientDataJSON),
        authenticatorData: toBase64(response.authenticatorData),
        signature: toBase64(response.signature),
        next: next || "",
      });
      if (!result.ok) throw new Error(await errorText(result));
      location.href = (await result.json()).redirect;
    } catch (error) {
      if (mediation !== "conditional") passkeyFailed(error);
    }
  }

  // Dictation: record in the browser, convert to 16 kHz WAV, and let the
  // server's speech model write it down.
  async function toWav16k(blob) {
    const context = new AudioContext();
    const decoded = await context.decodeAudioData(await blob.arrayBuffer());
    context.close();
    const offline = new OfflineAudioContext(1, Math.max(1, Math.ceil(decoded.duration * 16000)), 16000);
    const source = offline.createBufferSource();
    source.buffer = decoded;
    source.connect(offline.destination);
    source.start();
    const samples = (await offline.startRendering()).getChannelData(0);
    const buffer = new ArrayBuffer(44 + samples.length * 2);
    const view = new DataView(buffer);
    const text = (offset, value) => [...value].forEach((c, i) => view.setUint8(offset + i, c.charCodeAt(0)));
    text(0, "RIFF");
    view.setUint32(4, 36 + samples.length * 2, true);
    text(8, "WAVEfmt ");
    view.setUint32(16, 16, true);
    view.setUint16(20, 1, true);
    view.setUint16(22, 1, true);
    view.setUint32(24, 16000, true);
    view.setUint32(28, 32000, true);
    view.setUint16(32, 2, true);
    view.setUint16(34, 16, true);
    text(36, "data");
    view.setUint32(40, samples.length * 2, true);
    samples.forEach((sample, i) => view.setInt16(44 + i * 2, Math.max(-1, Math.min(1, sample)) * 0x7fff, true));
    return buffer;
  }

  function setupDictation(form) {
    const button = form.querySelector("[data-dictate]");
    const textarea = form.querySelector("textarea");
    if (!button || !textarea || !navigator.mediaDevices?.getUserMedia || !window.MediaRecorder) return;
    button.hidden = false;
    let recorder = null;
    let limit = 0;
    button.addEventListener("click", async () => {
      if (recorder) return recorder.stop();
      let stream;
      try {
        stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      } catch {
        return toast("Sideporch can't use the microphone. Allow it for this site to dictate.");
      }
      const chunks = [];
      recorder = new MediaRecorder(stream);
      recorder.ondataavailable = (event) => chunks.push(event.data);
      recorder.onstop = async () => {
        clearTimeout(limit);
        stream.getTracks().forEach((track) => track.stop());
        recorder = null;
        button.setAttribute("aria-pressed", "false");
        button.disabled = true;
        button.title = "Writing it down…";
        try {
          const wav = await toWav16k(new Blob(chunks, { type: chunks[0]?.type }));
          const response = await fetch("/speech/transcribe", {
            method: "POST",
            headers: { "content-type": "audio/wav", "x-sideporch-fetch": "1" },
            body: wav,
          });
          if (!response.ok) throw new Error(await errorText(response));
          const { text } = await response.json();
          if (text) {
            const before = textarea.value.slice(0, textarea.selectionStart);
            const after = textarea.value.slice(textarea.selectionEnd);
            const spaced = (before && !/\s$/.test(before) ? " " : "") + text;
            textarea.value = before + spaced + after;
            textarea.selectionStart = textarea.selectionEnd = before.length + spaced.length;
            textarea.dispatchEvent(new Event("input", { bubbles: true }));
            textarea.focus();
          } else {
            toast("No words heard. Try again a little closer to the microphone.");
          }
        } catch (error) {
          toast(error.message || "Dictation didn't work. Try again.");
        } finally {
          button.disabled = false;
          button.title = "Dictate";
        }
      };
      recorder.start();
      button.setAttribute("aria-pressed", "true");
      button.title = "Stop and write it down";
      toast("Listening. Press the microphone again when you're done.");
      limit = setTimeout(() => recorder?.stop(), 120_000);
    });
  }

  function setupPasskeys() {
    const supported = Boolean(window.PublicKeyCredential && navigator.credentials && isSecureContext);
    for (const note of document.querySelectorAll("[data-passkey-unsupported]")) note.hidden = supported;
    if (!supported) return;
    for (const area of document.querySelectorAll("[data-passkey-area]")) area.hidden = false;
    for (const button of document.querySelectorAll("[data-passkey-add]")) {
      button.hidden = false;
      button.addEventListener("click", addPasskey);
    }
    const buttons = document.querySelectorAll("[data-passkey-login]");
    for (const button of buttons) {
      button.addEventListener("click", () => signInWithPasskey(button.dataset.next));
    }
    // On the login form, offer passkeys in the username field's autofill.
    const username = document.querySelector('input[autocomplete~="webauthn"]');
    if (username && PublicKeyCredential.isConditionalMediationAvailable) {
      PublicKeyCredential.isConditionalMediationAvailable().then((available) => {
        if (available) signInWithPasskey(buttons[0]?.dataset.next, "conditional");
      });
    }
  }

  // The speech admin page follows downloads until they finish.
  const speechAdmin = document.querySelector("[data-speech-admin][data-downloading]");
  if (speechAdmin) {
    const poll = setInterval(async () => {
      const response = await fetch("/admin/speech/status").catch(() => null);
      if (!response?.ok) return;
      const { models } = await response.json();
      let running = false;
      for (const model of models) {
        running ||= model.running;
        const card = speechAdmin.querySelector(`[data-model="${model.key}"]`);
        const bar = card?.querySelector("[data-progress]");
        if (bar) bar.value = model.received;
        const label = card?.querySelector("[data-progress-text]");
        if (label) label.textContent = `Downloading, ${Math.ceil(model.received / 1048576)} of ${Math.ceil(model.total / 1048576)} MB`;
      }
      if (!running) {
        clearInterval(poll);
        location.reload();
      }
    }, 1500);
  }

  for (const link of document.querySelectorAll("a[data-nav-link]")) {
    if (link.pathname === location.pathname) link.setAttribute("aria-current", "page");
  }
  localizeTimes(document);
  markOwnReactions(document);
  drawDiagrams(document);
  setupCopyButtons();
  setupPasskeys();
  setupQuickSearch();
  setupReactions();
  setupPush().catch((error) => console.warn("sideporch: notifications unavailable", error));
  setupInstallHint();
  updateAppBadge();
  clearNotifications().catch(() => {});
  document.addEventListener("visibilitychange", () => clearNotifications().catch(() => {}));
  if (gifPicker) {
    for (const button of document.querySelectorAll("[data-gif-button]")) button.hidden = false;
  }
  document.querySelectorAll("form[data-composer]").forEach(setupComposer);
  document.querySelectorAll("form[data-composer]").forEach(setupDictation);
  scrollToEnd(document.getElementById("scroller"));
  scrollToEnd(document.getElementById("thread-scroller"));
  if (app) {
    connect();
    const composer = document.querySelector(app.dataset.thread ? "aside form[data-composer] textarea" : "form[data-composer] textarea");
    if (composer && matchMedia("(pointer: fine)").matches) composer.focus();
  }
})();
