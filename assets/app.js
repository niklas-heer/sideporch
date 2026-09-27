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
    markOwnReactions(item);
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

  function handleEvent(event, socket) {
    if (event.type === "resync") {
      location.reload();
      return;
    }
    if (event.type === "reactions") {
      replaceReactions(event.message_id, event.html);
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
    const sendVisibility = () => {
      if (socket.readyState === WebSocket.OPEN) {
        socket.send(JSON.stringify({ type: "visibility", visible: document.visibilityState === "visible" }));
      }
    };
    socket.addEventListener("open", () => {
      opened = true;
      if (attempt > 0) location.reload();
      sendVisibility();
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

    textarea.addEventListener("input", () => {
      resize();
      sessionStorage.setItem(draftKey, textarea.value);
    });
    const showFiles = () => {
      fileList.replaceChildren(
        ...[...fileInput.files].map((file) => {
          const item = document.createElement("li");
          item.className = "rounded-md bg-screen px-2 py-0.5 dark:bg-night-2";
          item.textContent = file.name;
          return item;
        }),
      );
      fileList.classList.toggle("hidden", fileInput.files.length === 0);
      fileList.classList.toggle("flex", fileInput.files.length > 0);
    };
    fileInput?.addEventListener("change", showFiles);

    textarea.addEventListener("keydown", (event) => {
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
    document.addEventListener("click", (event) => {
      const trigger = event.target.closest("a[data-react]");
      if (!trigger) return;
      event.preventDefault();
      picker.dataset.message = trigger.dataset.react;
      picker.showPopover();
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

  localizeTimes(document);
  markOwnReactions(document);
  setupCopyButtons();
  setupReactions();
  setupPush().catch((error) => console.warn("sideporch: notifications unavailable", error));
  document.querySelectorAll("form[data-composer]").forEach(setupComposer);
  scrollToEnd(document.getElementById("scroller"));
  scrollToEnd(document.getElementById("thread-scroller"));
  if (app) {
    connect();
    const composer = document.querySelector(app.dataset.thread ? "aside form[data-composer] textarea" : "form[data-composer] textarea");
    if (composer && matchMedia("(pointer: fine)").matches) composer.focus();
  }
})();
