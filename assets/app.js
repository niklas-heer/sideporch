// Sideporch's only script. Pages work without it; it adds live updates,
// sending without a page reload, local times, drafts and copy buttons.
"use strict";

(() => {
  const app = document.getElementById("app");
  const GROUP_WINDOW_MS = 5 * 60 * 1000;
  const timeFormat = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit" });
  const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "medium" });
  const fullFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "full", timeStyle: "short" });

  function localizeTimes(root) {
    for (const time of root.querySelectorAll("time[datetime]")) {
      const date = new Date(time.getAttribute("datetime"));
      if (Number.isNaN(date.getTime())) continue;
      time.textContent = time.dataset.format === "date" ? dateFormat.format(date) : timeFormat.format(date);
      time.title = fullFormat.format(date);
    }
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
    list.append(item);
    if (follow) scrollToEnd(scroller);
  }

  function replyLabel(count) {
    return count === 1 ? "1 reply" : `${count} replies`;
  }

  function handleEvent(event, socket) {
    if (event.type === "resync") {
      location.reload();
      return;
    }
    if (event.type !== "message") return;
    const here = app && app.dataset.channel === String(event.channel_id);
    if (!here) {
      const link = document.querySelector(`[data-channel-link="${event.channel_id}"]`);
      if (link && event.author !== `u:${app?.dataset.me}`) link.dataset.unread = "";
      return;
    }
    if (event.parent_id === null) {
      const list = document.getElementById("messages");
      if (list) appendMessage(list, event.html, document.getElementById("scroller"));
    } else {
      if (app.dataset.thread === String(event.parent_id)) {
        const replies = document.getElementById("replies");
        if (replies) appendMessage(replies, event.html, document.getElementById("thread-scroller"));
        const count = document.getElementById("thread-count");
        if (count && event.reply_count !== null) count.textContent = replyLabel(event.reply_count);
      }
      const link = document.querySelector(`[data-reply-count="${event.parent_id}"]`);
      if (link && event.reply_count !== null) {
        link.classList.remove("hidden");
        link.classList.add("inline-flex");
        const label = link.querySelector("span");
        if (label) label.textContent = replyLabel(event.reply_count);
      }
    }
    if (document.visibilityState === "visible" && socket.readyState === WebSocket.OPEN) {
      socket.send(JSON.stringify({ type: "read", channel_id: event.channel_id, message_id: event.id }));
    }
  }

  // Live updates. After a dropped connection the page reloads to catch up
  // on anything it missed; drafts survive in session storage.
  function connect(attempt = 0) {
    const protocol = location.protocol === "https:" ? "wss:" : "ws:";
    const socket = new WebSocket(`${protocol}//${location.host}/ws`);
    let opened = false;
    socket.addEventListener("open", () => {
      opened = true;
      if (attempt > 0) location.reload();
    });
    socket.addEventListener("message", (message) => {
      try {
        handleEvent(JSON.parse(message.data), socket);
      } catch (error) {
        console.error("sideporch: bad event", error);
      }
    });
    socket.addEventListener("close", () => {
      const delay = Math.min(30000, 1000 * 2 ** Math.min(attempt, 5));
      setTimeout(() => connect(opened ? 1 : attempt + 1), delay);
    });
  }

  function setupComposer(form) {
    const textarea = form.querySelector("textarea");
    const button = form.querySelector("button[type=submit]");
    const error = form.querySelector("[data-composer-error]");
    const draftKey = `sideporch:draft:${form.action}:${form.elements.parent_id?.value ?? ""}`;
    textarea.value = sessionStorage.getItem(draftKey) ?? "";

    const resize = () => {
      textarea.style.height = "auto";
      textarea.style.height = `${textarea.scrollHeight}px`;
    };
    resize();

    textarea.addEventListener("input", () => {
      resize();
      sessionStorage.setItem(draftKey, textarea.value);
    });
    textarea.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
        event.preventDefault();
        form.requestSubmit();
      }
    });

    form.addEventListener("submit", async (event) => {
      event.preventDefault();
      if (!textarea.value.trim() || button.disabled) return;
      button.disabled = true;
      error.classList.add("hidden");
      try {
        const response = await fetch(form.action, {
          method: "POST",
          headers: { "x-sideporch-fetch": "1" },
          body: new URLSearchParams(new FormData(form)),
        });
        if (!response.ok) throw new Error(`status ${response.status}`);
        textarea.value = "";
        sessionStorage.removeItem(draftKey);
        resize();
      } catch {
        error.textContent = "Your message wasn't sent. Check your connection and try again.";
        error.classList.remove("hidden");
      } finally {
        button.disabled = false;
        textarea.focus();
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

  localizeTimes(document);
  setupCopyButtons();
  document.querySelectorAll("form[data-composer]").forEach(setupComposer);
  scrollToEnd(document.getElementById("scroller"));
  scrollToEnd(document.getElementById("thread-scroller"));
  if (app) {
    connect();
    const composer = document.querySelector(app.dataset.thread ? "aside form[data-composer] textarea" : "form[data-composer] textarea");
    if (composer && matchMedia("(pointer: fine)").matches) composer.focus();
  }
})();
