// Sideporch's only page script. Pages work without it; it adds live
// updates, sending without a page reload, reactions in place, the emoji
// picker, push notifications, local times, drafts and copy buttons.
"use strict";

(() => {
  const app = document.getElementById("app");
  const GROUP_WINDOW_MS = 5 * 60 * 1000;
  const timeFormat = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit" });
  const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "medium" });
  const fullFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "full", timeStyle: "short" });
  const dateTimeFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

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
        const dark = matchMedia("(prefers-color-scheme: dark)").matches;
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

  // Reactions are rendered once for everyone; highlight the viewer's own.
  function markOwnReactions(root) {
    const me = app?.dataset.me;
    for (const button of root.querySelectorAll("[data-users]")) {
      const mine = button.dataset.users.split(",").includes(me);
      button.setAttribute("aria-pressed", mine ? "true" : "false");
    }
  }

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
  function renderTyping(form) {
    const label = form.querySelector("[data-typing]");
    if (!label) return;
    const names = [...(typists.get(form)?.values() ?? [])].map((person) => person.name);
    label.textContent =
      names.length === 0 ? "" :
      names.length === 1 ? `${names[0]} is typing…` :
      names.length === 2 ? `${names[0]} and ${names[1]} are typing…` :
      "Several people are typing…";
  }
  function showTyping(event) {
    if (String(event.user_id) === app?.dataset.me || app?.dataset.channel !== String(event.channel_id)) return;
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
    if (!here) {
      const link = document.querySelector(`[data-channel-link="${event.channel_id}"]`);
      if (link && link.dataset.muted === undefined && event.author !== `u:${app?.dataset.me}`) link.dataset.unread = "";
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
      if (text && !text.startsWith("/") && Date.now() - typingSent > 3000 && liveSocket?.readyState === WebSocket.OPEN) {
        typingSent = Date.now();
        const parent = form.elements.parent_id?.value;
        liveSocket.send(JSON.stringify({ type: "typing", channel_id: Number(app.dataset.channel), parent_id: parent ? Number(parent) : null }));
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
        if (mine) {
          event.preventDefault();
          startEdit(mine);
          return;
        }
      }
      if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
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
    const admin = app?.dataset.admin !== undefined;
    const deleted = item.dataset.deleted !== undefined;
    const entries = [];
    if (mine && !deleted) entries.push(["Edit message", () => startEdit(item)]);
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
    if ((mine || admin) && !deleted) {
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
  const menuExtras = [
    (item, base) => (item.dataset.deleted === undefined ? ["Remind me in 1 hour", () => remindAbout(base, "in 1 hour")] : null),
    (item, base) => (item.dataset.deleted === undefined ? ["Remind me tomorrow", () => remindAbout(base, "tomorrow")] : null),
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
      } else if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
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

  function base64UrlToBytes(text) {
    const base64 = (text + "===".slice((text.length + 3) % 4)).replace(/-/g, "+").replace(/_/g, "/");
    return Uint8Array.from(atob(base64), (char) => char.charCodeAt(0));
  }

  // The bell in the sidebar turns push notifications on and off for this
  // browser. On iPhone and iPad this needs Sideporch added to the home screen.
  async function setupPush() {
    const toggle = document.querySelector("[data-push-toggle]");
    if (!toggle || !("serviceWorker" in navigator) || !("PushManager" in window) || !window.Notification) return;
    const registration = await navigator.serviceWorker.register("/sw.js");
    const show = (subscribed) => toggle.setAttribute("aria-pressed", subscribed ? "true" : "false");
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

  for (const link of document.querySelectorAll("a[data-nav-link]")) {
    if (link.pathname === location.pathname) link.setAttribute("aria-current", "page");
  }
  localizeTimes(document);
  markOwnReactions(document);
  drawDiagrams(document);
  setupCopyButtons();
  setupReactions();
  setupPush().catch((error) => console.warn("sideporch: notifications unavailable", error));
  if (gifPicker) {
    for (const button of document.querySelectorAll("[data-gif-button]")) button.hidden = false;
  }
  document.querySelectorAll("form[data-composer]").forEach(setupComposer);
  scrollToEnd(document.getElementById("scroller"));
  scrollToEnd(document.getElementById("thread-scroller"));
  if (app) {
    connect();
    const composer = document.querySelector(app.dataset.thread ? "aside form[data-composer] textarea" : "form[data-composer] textarea");
    if (composer && matchMedia("(pointer: fine)").matches) composer.focus();
  }
})();
