// The automation editor. It turns the plain script textarea into a code
// editor: highlighting, line numbers, live lint markers, completions for the
// sideporch API, formatting, test runs and AI help. Without this script the
// textarea and the save button still work.
"use strict";

(() => {
  const textarea = document.querySelector("textarea[data-lua-editor]");
  const form = document.getElementById("automation-form");
  if (!textarea || !form) return;

  const INDENT = "  ";
  const api = JSON.parse(document.getElementById("lua-api")?.textContent || "[]");
  const automationId = form.dataset.automationId ? Number(form.dataset.automationId) : null;
  const isLibrary = form.dataset.kind === "library";
  const nameInput = document.getElementById("automation-name");

  // Layout: a gutter with line numbers, and the textarea over a highlighted copy.
  const root = textarea.closest("[data-editor]");
  const gutter = document.createElement("div");
  gutter.className = "code-gutter";
  gutter.setAttribute("aria-hidden", "true");
  const highlight = document.createElement("pre");
  highlight.className = "code-highlight";
  highlight.setAttribute("aria-hidden", "true");
  const area = document.createElement("div");
  area.className = "code-area";
  textarea.classList.remove("field", "text-sm", "leading-relaxed");
  textarea.classList.add("code-input");
  textarea.setAttribute("wrap", "off");
  root.append(gutter, area);
  area.append(highlight, textarea);
  root.classList.add("is-enhanced");

  const popup = document.createElement("ul");
  popup.className = "code-completions";
  popup.id = "code-completions";
  popup.setAttribute("role", "listbox");
  popup.hidden = true;
  area.append(popup);

  for (const hidden of document.querySelectorAll("[data-editor-tools], [data-editor-hint], [data-test-panel], [data-ai-panel]")) {
    hidden.hidden = false;
  }
  const status = document.querySelector("[data-editor-status]");
  const problemList = document.querySelector("[data-problems]");

  // --- Highlighting ------------------------------------------------------

  const KEYWORDS = new Set(
    "and break do else elseif end false for function goto if in local nil not or repeat return then true until while".split(" "),
  );
  const BUILTINS = new Set(
    "assert error getmetatable ipairs next pairs pcall print rawequal rawget rawlen rawset select setmetatable tonumber tostring type xpcall warn string table math utf8 coroutine self".split(" "),
  );

  // Splits Lua source into [start, end, class] tokens; plain text is skipped.
  function tokenize(text) {
    const tokens = [];
    const length = text.length;
    let i = 0;
    const longBracket = (at) => {
      const match = /^\[(=*)\[/.exec(text.slice(at, at + 64));
      return match ? match[1].length : -1;
    };
    const closeLong = (from, level) => {
      const end = text.indexOf("]" + "=".repeat(level) + "]", from);
      return end === -1 ? length : end + level + 2;
    };
    let afterSideporch = false;
    while (i < length) {
      const ch = text[i];
      if (ch === "-" && text[i + 1] === "-") {
        const level = text[i + 2] === "[" ? longBracket(i + 2) : -1;
        const end = level >= 0 ? closeLong(i + 4 + level, level) : (text.indexOf("\n", i) + 1 || length + 1) - 1;
        tokens.push([i, end, "tok-c"]);
        i = end;
        continue;
      }
      if (ch === '"' || ch === "'") {
        let j = i + 1;
        while (j < length && text[j] !== ch && text[j] !== "\n") j += text[j] === "\\" ? 2 : 1;
        const end = Math.min(j + 1, length);
        tokens.push([i, end, "tok-s"]);
        i = end;
        continue;
      }
      if (ch === "[") {
        const level = longBracket(i);
        if (level >= 0) {
          const end = closeLong(i + 2 + level, level);
          tokens.push([i, end, "tok-s"]);
          i = end;
          continue;
        }
      }
      if (/[0-9]/.test(ch) || (ch === "." && /[0-9]/.test(text[i + 1] || ""))) {
        const match = /^(0[xX][0-9a-fA-F.]+([pP][+-]?\d+)?|\d*\.?\d+([eE][+-]?\d+)?)/.exec(text.slice(i, i + 64));
        const end = i + (match ? match[0].length : 1);
        tokens.push([i, end, "tok-n"]);
        i = end;
        continue;
      }
      if (/[A-Za-z_]/.test(ch)) {
        const match = /^[A-Za-z_][A-Za-z0-9_]*/.exec(text.slice(i, i + 128));
        const word = match[0];
        const end = i + word.length;
        let kind = null;
        if (KEYWORDS.has(word)) kind = "tok-k";
        else if (word === "sideporch") kind = "tok-api";
        else if (afterSideporch) kind = "tok-api";
        else if (BUILTINS.has(word) && text[i - 1] !== ".") kind = "tok-b";
        else if (/^\s*\(/.test(text.slice(end, end + 8))) kind = "tok-f";
        if (kind) tokens.push([i, end, kind]);
        afterSideporch = (word === "sideporch" || afterSideporch) && text[end] === ".";
        i = end;
        continue;
      }
      if (ch !== ".") afterSideporch = false;
      i += 1;
    }
    return tokens;
  }

  const escapeHtml = (value) =>
    value.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

  // HTML for the highlight layer: tokens plus wavy lint underlines.
  function render(text, diagnostics) {
    const tokens = tokenize(text);
    const marks = diagnostics.map((d) => {
      let end = Math.max(d.end, d.start + 1);
      if (d.start >= text.length) end = d.start;
      return [Math.min(d.start, text.length), Math.min(end, text.length), d.severity];
    });
    const cuts = new Set([0, text.length]);
    for (const [start, end] of tokens) cuts.add(start).add(end);
    for (const [start, end] of marks) cuts.add(start).add(end);
    const points = [...cuts].sort((a, b) => a - b);
    let html = "";
    let t = 0;
    for (let p = 0; p < points.length - 1; p += 1) {
      const start = points[p];
      const end = points[p + 1];
      if (start === end) continue;
      while (t < tokens.length && tokens[t][1] <= start) t += 1;
      const token = t < tokens.length && tokens[t][0] <= start ? tokens[t][2] : "";
      const mark = marks.find(([s, e]) => s <= start && end <= e);
      const classes = [token, mark ? `lint-${mark[2]}` : ""].filter(Boolean).join(" ");
      const piece = escapeHtml(text.slice(start, end));
      html += classes ? `<span class="${classes}">${piece}</span>` : piece;
    }
    // A trailing newline needs something after it to take up a line.
    return html + "\n ";
  }

  let diagnostics = [];

  function paint() {
    const text = textarea.value;
    highlight.innerHTML = render(text, diagnostics);
    const lines = text.split("\n").length;
    const flagged = new Map();
    for (const d of diagnostics) {
      if (flagged.get(d.line) !== "error") flagged.set(d.line, d.severity);
    }
    let numbers = "";
    for (let line = 1; line <= lines; line += 1) {
      const severity = flagged.get(line);
      numbers += severity ? `<span class="gutter-${severity}">${line}</span>\n` : `${line}\n`;
    }
    gutter.innerHTML = numbers;
    syncScroll();
  }

  function syncScroll() {
    highlight.scrollTop = textarea.scrollTop;
    highlight.scrollLeft = textarea.scrollLeft;
    gutter.scrollTop = textarea.scrollTop;
  }

  let painting = false;
  function schedulePaint() {
    if (painting) return;
    painting = true;
    requestAnimationFrame(() => {
      painting = false;
      paint();
    });
  }

  // --- Talking to the server ----------------------------------------------

  async function postJson(url, body) {
    const response = await fetch(url, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
    if (!response.ok) {
      const text = await response.text();
      const message = new DOMParser().parseFromString(text, "text/html").querySelector("main p, p")?.textContent;
      throw new Error(message || `The server answered ${response.status}.`);
    }
    return response.json();
  }

  // --- Linting -------------------------------------------------------------

  let lintTimer = 0;
  let lintRound = 0;

  function scheduleLint() {
    clearTimeout(lintTimer);
    lintTimer = setTimeout(lint, 350);
  }

  async function lint() {
    const round = ++lintRound;
    const source = textarea.value;
    try {
      const result = await postJson("/automations/lint", { source });
      if (round !== lintRound) return;
      showDiagnostics(result.diagnostics);
    } catch (error) {
      if (round === lintRound) setStatus(`Could not check the script: ${error.message}`);
    }
  }

  function setStatus(text, tone = "") {
    status.textContent = text;
    status.dataset.tone = tone;
  }

  function showDiagnostics(found) {
    diagnostics = found;
    const errors = found.filter((d) => d.severity === "error").length;
    const warnings = found.length - errors;
    if (found.length === 0) setStatus("No problems", "ok");
    else {
      const parts = [];
      if (errors) parts.push(`${errors} ${errors === 1 ? "error" : "errors"}`);
      if (warnings) parts.push(`${warnings} ${warnings === 1 ? "warning" : "warnings"}`);
      setStatus(parts.join(", "), errors ? "error" : "warning");
    }
    problemList.replaceChildren(
      ...found.map((d) => {
        const item = document.createElement("li");
        const button = document.createElement("button");
        button.type = "button";
        button.className = `problem problem-${d.severity}`;
        button.textContent = `Line ${d.line}:${d.column}  ${d.message}`;
        button.title = d.code;
        button.addEventListener("click", () => jumpTo(d.start, d.end));
        item.append(button);
        return item;
      }),
    );
    problemList.hidden = found.length === 0;
    paint();
  }

  // --- Editing helpers -----------------------------------------------------

  // Replaces a range through the browser's editing commands, so undo works.
  function replaceRange(start, end, text) {
    textarea.focus();
    textarea.setSelectionRange(start, end);
    if (!document.execCommand("insertText", false, text)) {
      textarea.setRangeText(text, start, end, "end");
      textarea.dispatchEvent(new Event("input"));
    }
  }

  function replaceAll(text) {
    if (text === textarea.value) return;
    const caret = textarea.selectionStart;
    replaceRange(0, textarea.value.length, text);
    textarea.setSelectionRange(Math.min(caret, text.length), Math.min(caret, text.length));
  }

  function lineStart(text, at) {
    return text.lastIndexOf("\n", at - 1) + 1;
  }

  function lineHeight() {
    return parseFloat(getComputedStyle(textarea).lineHeight) || 20;
  }

  function jumpTo(start, end = start) {
    textarea.focus();
    textarea.setSelectionRange(start, end);
    const line = textarea.value.slice(0, start).split("\n").length - 1;
    textarea.scrollTop = Math.max(0, line * lineHeight() - textarea.clientHeight / 3);
    syncScroll();
  }

  function jumpToLine(line) {
    const lines = textarea.value.split("\n");
    let at = 0;
    for (let i = 0; i < Math.min(line - 1, lines.length); i += 1) at += lines[i].length + 1;
    jumpTo(at, at + (lines[line - 1] || "").length);
  }

  // Indents or outdents every line the selection touches.
  function shiftLines(outdent) {
    const text = textarea.value;
    const { selectionStart: start, selectionEnd: end } = textarea;
    const from = lineStart(text, start);
    const blockEnd = end > start && text[end - 1] === "\n" ? end - 1 : end;
    const toBreak = text.indexOf("\n", blockEnd);
    const to = toBreak === -1 ? text.length : toBreak;
    const lines = text.slice(from, to).split("\n");
    let firstShift = 0;
    const changed = lines.map((line, index) => {
      if (!outdent) {
        if (index === 0) firstShift = INDENT.length;
        return INDENT + line;
      }
      const remove = line.startsWith(INDENT) ? INDENT.length : line.startsWith(" ") || line.startsWith("\t") ? 1 : 0;
      if (index === 0) firstShift = -remove;
      return line.slice(remove);
    });
    const replacement = changed.join("\n");
    replaceRange(from, to, replacement);
    const newStart = Math.max(from, start + firstShift);
    textarea.setSelectionRange(newStart, start === end ? newStart : from + replacement.length);
  }

  function toggleComment() {
    const text = textarea.value;
    const { selectionStart: start, selectionEnd: end } = textarea;
    const from = lineStart(text, start);
    const toBreak = text.indexOf("\n", end > start && text[end - 1] === "\n" ? end - 1 : end);
    const to = toBreak === -1 ? text.length : toBreak;
    const lines = text.slice(from, to).split("\n");
    const commented = lines.filter((line) => line.trim()).every((line) => /^\s*--/.test(line));
    const replacement = lines
      .map((line) => {
        if (!line.trim()) return line;
        if (commented) return line.replace(/^(\s*)-- ?/, "$1");
        const indent = /^\s*/.exec(line)[0];
        return `${indent}-- ${line.slice(indent.length)}`;
      })
      .join("\n");
    replaceRange(from, to, replacement);
    textarea.setSelectionRange(from, from + replacement.length);
  }

  const OPENS = /(\bthen|\bdo|\belse|\brepeat|\bfunction\s*[\w.:]*\s*\([^)]*\)|[{(])\s*(--.*)?$/;

  // Moves a closing word back one level once it is typed at a line's start:
  // below a block's body, or right below the line that opened the block.
  let dedenting = false;
  function dedentCloser(event) {
    if (dedenting || event.inputType !== "insertText") return;
    const text = textarea.value;
    const caret = textarea.selectionStart;
    const from = lineStart(text, caret);
    const match = /^( +)(end|else|elseif|until|\})$/.exec(text.slice(from, caret));
    if (!match || /\w/.test(text[caret] || "")) return;
    const indent = match[1].length;
    const previous = text.slice(0, Math.max(0, from - 1)).split("\n").reverse().find((line) => line.trim());
    if (previous === undefined) return;
    const previousIndent = /^ */.exec(previous)[0].length;
    const deeper = OPENS.test(previous) ? indent > previousIndent : indent >= previousIndent;
    if (!deeper) return;
    const remove = Math.min(INDENT.length, indent);
    dedenting = true;
    replaceRange(from, from + remove, "");
    dedenting = false;
    textarea.setSelectionRange(caret - remove, caret - remove);
  }

  // Keeps the indentation on Enter, one level deeper after an opening line.
  function newline() {
    const text = textarea.value;
    const start = textarea.selectionStart;
    const before = text.slice(lineStart(text, start), start);
    const indent = /^\s*/.exec(before)[0];
    const opens = OPENS.test(before);
    const after = text.slice(start, text.indexOf("\n", start) === -1 ? text.length : text.indexOf("\n", start));
    if (opens && /^\s*[)}]/.test(after)) {
      replaceRange(start, textarea.selectionEnd, `\n${indent}${INDENT}\n${indent}`);
      const caret = start + 1 + indent.length + INDENT.length;
      textarea.setSelectionRange(caret, caret);
      return;
    }
    replaceRange(start, textarea.selectionEnd, `\n${indent}${opens ? INDENT : ""}`);
  }

  // --- Completions ---------------------------------------------------------

  let choices = [];
  let chosen = 0;
  let completionStart = 0;

  function completionContext() {
    const text = textarea.value;
    const caret = textarea.selectionStart;
    const match = /sideporch\.((?:json\.)?[A-Za-z_]*)$/.exec(text.slice(Math.max(0, caret - 64), caret));
    if (!match) return null;
    return { typed: match[1], start: caret - match[1].length };
  }

  function updateCompletions(force = false) {
    const context = completionContext();
    if (!context || (!force && context.typed === "" && !/\.$/.test(textarea.value.slice(0, textarea.selectionStart)))) {
      closeCompletions();
      return;
    }
    choices = api.filter((entry) => entry.name.startsWith(context.typed));
    if (choices.length === 0 || (choices.length === 1 && choices[0].name === context.typed)) {
      closeCompletions();
      return;
    }
    completionStart = context.start;
    chosen = 0;
    popup.replaceChildren(
      ...choices.map((entry, index) => {
        const item = document.createElement("li");
        item.id = `completion-${index}`;
        item.setAttribute("role", "option");
        const signature = document.createElement("code");
        signature.textContent = entry.signature.replace(/^sideporch\./, "");
        const doc = document.createElement("span");
        doc.textContent = entry.doc.replace(/`/g, "");
        item.append(signature, doc);
        item.addEventListener("mousedown", (event) => {
          event.preventDefault();
          chosen = index;
          acceptCompletion();
        });
        return item;
      }),
    );
    placePopup();
    popup.hidden = false;
    textarea.setAttribute("aria-expanded", "true");
    textarea.setAttribute("aria-controls", popup.id);
    markChosen();
  }

  function placePopup() {
    const text = textarea.value.slice(0, textarea.selectionStart);
    const line = text.split("\n").length - 1;
    const column = text.length - lineStart(text, text.length);
    const style = getComputedStyle(textarea);
    const probe = document.createElement("span");
    probe.textContent = "0".repeat(20);
    probe.style.cssText = `font:${style.font};position:absolute;visibility:hidden;white-space:pre`;
    document.body.append(probe);
    const charWidth = probe.getBoundingClientRect().width / 20;
    probe.remove();
    const top = parseFloat(style.paddingTop) + (line + 1) * lineHeight() - textarea.scrollTop;
    const left = parseFloat(style.paddingLeft) + column * charWidth - textarea.scrollLeft;
    popup.style.top = `${Math.max(0, top + 2)}px`;
    popup.style.left = `${Math.max(0, Math.min(left, area.clientWidth - 280))}px`;
  }

  function markChosen() {
    for (const [index, item] of [...popup.children].entries()) {
      item.setAttribute("aria-selected", index === chosen ? "true" : "false");
      if (index === chosen) item.scrollIntoView({ block: "nearest" });
    }
    textarea.setAttribute("aria-activedescendant", `completion-${chosen}`);
  }

  function closeCompletions() {
    popup.hidden = true;
    choices = [];
    textarea.setAttribute("aria-expanded", "false");
    textarea.removeAttribute("aria-activedescendant");
  }

  function acceptCompletion() {
    const entry = choices[chosen];
    if (!entry) return;
    closeCompletions();
    const snippet = entry.snippet;
    const text = textarea.value;
    const indent = /^\s*/.exec(text.slice(lineStart(text, completionStart), completionStart))[0];
    const body = snippet.replace(/\n/g, `\n${indent}`);
    const cursor = body.indexOf("$0");
    const insert = body.replace("$0", "");
    replaceRange(completionStart, textarea.selectionStart, insert);
    const caret = completionStart + (cursor === -1 ? insert.length : cursor);
    textarea.setSelectionRange(caret, caret);
  }

  // --- Keys ----------------------------------------------------------------

  let tabLeaves = false;

  textarea.addEventListener("keydown", (event) => {
    const mod = event.metaKey || event.ctrlKey;
    if (!popup.hidden) {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        chosen = (chosen + (event.key === "ArrowDown" ? 1 : choices.length - 1)) % choices.length;
        markChosen();
        return;
      }
      if (event.key === "Enter" || event.key === "Tab") {
        event.preventDefault();
        acceptCompletion();
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        closeCompletions();
        return;
      }
    }
    if (event.key === "Escape") {
      tabLeaves = true;
      return;
    }
    if (event.key === "Tab" && !mod && !event.altKey) {
      if (tabLeaves) return;
      event.preventDefault();
      const { selectionStart: start, selectionEnd: end } = textarea;
      if (event.shiftKey || textarea.value.slice(start, end).includes("\n")) shiftLines(event.shiftKey);
      else replaceRange(start, end, INDENT);
      return;
    }
    tabLeaves = false;
    if (event.key === "Enter" && !mod && !event.shiftKey && !event.altKey) {
      event.preventDefault();
      newline();
    } else if (mod && event.key.toLowerCase() === "s") {
      event.preventDefault();
      form.requestSubmit();
    } else if (mod && event.key === "Enter") {
      event.preventDefault();
      runTest();
    } else if (mod && event.key === "/") {
      event.preventDefault();
      toggleComment();
    } else if (event.ctrlKey && event.key === " ") {
      event.preventDefault();
      updateCompletions(true);
    } else if (event.shiftKey && event.altKey && event.code === "KeyF") {
      event.preventDefault();
      format();
    }
  });

  let dirty = false;
  textarea.addEventListener("input", (event) => {
    dirty = true;
    dedentCloser(event);
    schedulePaint();
    scheduleLint();
    if (event.inputType?.startsWith("insert") || event.inputType?.startsWith("delete")) updateCompletions();
  });
  textarea.addEventListener("scroll", () => {
    syncScroll();
    if (!popup.hidden) placePopup();
  });
  textarea.addEventListener("blur", () => setTimeout(closeCompletions, 150));
  textarea.addEventListener("click", closeCompletions);
  nameInput?.addEventListener("input", () => {
    dirty = true;
  });
  form.addEventListener("submit", () => {
    dirty = false;
  });
  window.addEventListener("beforeunload", (event) => {
    if (dirty) event.preventDefault();
  });

  // --- Formatting ----------------------------------------------------------

  async function format() {
    setStatus("Formatting…");
    try {
      const result = await postJson("/automations/format", { source: textarea.value });
      if (result.source !== null && result.source !== undefined) {
        replaceAll(result.source);
        await lint();
      } else {
        showDiagnostics(result.diagnostics);
      }
    } catch (error) {
      setStatus(`Could not format: ${error.message}`, "error");
    }
  }

  // --- Test runs -----------------------------------------------------------

  const testPanel = document.querySelector("[data-test-panel]");
  const testForm = document.querySelector("[data-test-form]");
  const testOutput = document.querySelector("[data-test-output]");

  function showTestFields() {
    const kind = testForm.elements.kind.value;
    for (const field of testForm.querySelectorAll("[data-for]")) {
      field.hidden = !field.dataset.for.split(" ").includes(kind);
    }
  }

  function element(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  }

  // An error line such as "line 3: …" becomes a link to that line.
  function errorLine(message) {
    const node = element("p", "test-error");
    const match = /^line (\d+): /.exec(message);
    if (match) {
      const link = element("button", "test-line", `Line ${match[1]}`);
      link.type = "button";
      link.addEventListener("click", () => jumpToLine(Number(match[1])));
      node.append(link, ` ${message.slice(match[0].length)}`);
    } else {
      node.textContent = message;
    }
    return node;
  }

  async function runTest() {
    testPanel.scrollIntoView({ block: "nearest", behavior: "smooth" });
    const fields = testForm.elements;
    const kind = fields.kind.value;
    const trigger = { kind };
    if (kind === "message" || kind === "reaction") {
      trigger.text = fields.text.value;
      trigger.channel = fields.channel.value;
    }
    if (kind === "reaction") {
      trigger.emoji = fields.emoji.value;
      trigger.added = fields.added.checked;
    }
    if (kind === "webhook") {
      trigger.method = fields.method.value;
      trigger.path = fields.path.value;
      trigger.body = fields.body.value;
    }
    if (kind === "command") {
      trigger.text = fields.command.value;
      trigger.channel = fields.channel.value;
    }
    if (kind === "member_joined") trigger.user = fields.user.value;
    if (kind === "channel_created") trigger.channel = fields.new_channel.value;
    testOutput.replaceChildren(element("p", "text-sm text-muted", "Running…"));
    try {
      const report = await postJson("/automations/test", {
        source: textarea.value,
        name: nameInput?.value || "",
        automation_id: automationId,
        trigger,
        http: fields.http.checked,
      });
      const parts = [];
      const triggers = report.triggers;
      const registered = [
        ...triggers.events.map((event) => `on ${event.description}`),
        ...triggers.schedules.map((schedule) => schedule.description),
        ...triggers.commands.map((command) => `/${command.name}`),
        ...(triggers.webhook ? ["a webhook"] : []),
      ];
      const cost =
        report.instructions < 1000
          ? "under 1,000 instructions"
          : `about ${report.instructions.toLocaleString()} instructions`;
      parts.push(
        element(
          "p",
          `test-summary ${report.ok ? "is-ok" : "is-error"}`,
          report.ok
            ? `✓ Ran ${report.called} ${report.called === 1 ? "handler" : "handlers"} in ${report.duration_ms.toFixed(1)} ms`
            : "✗ The script failed",
        ),
      );
      const handlerText = isLibrary
        ? report.exports.length
          ? `Exports ${report.exports.join(", ")}`
          : "Exports nothing: return a table from the library"
        : registered.length
          ? `Listens to ${registered.join("; ")}`
          : "Listens to nothing";
      parts.push(element("p", "test-meta", `${handlerText}; used ${cost}.`));
      if (report.ok && report.called === 0 && kind !== "load") {
        parts.push(element("p", "test-meta", "No handler matched this event."));
      }
      if (report.log.length) {
        const log = element("pre", "test-log");
        for (const line of report.log) {
          log.append(element("span", line.startsWith("→ ") ? "test-action" : "", line), "\n");
        }
        parts.push(log);
      }
      for (const text of report.responses) {
        parts.push(element("p", "test-meta", "Private answer to the person who ran the command"), element("pre", "test-log", text));
      }
      if (report.error) parts.push(errorLine(report.error));
      if (report.response) {
        const response = element("pre", "test-log");
        response.textContent = `HTTP ${report.response.status} ${report.response.content_type}\n\n${report.response.body}`;
        parts.push(element("p", "test-meta", "Webhook response"), response);
      }
      testOutput.replaceChildren(...parts);
    } catch (error) {
      testOutput.replaceChildren(errorLine(error.message));
    }
  }

  testForm.elements.kind.addEventListener("change", showTestFields);
  testForm.addEventListener("submit", (event) => {
    event.preventDefault();
    runTest();
  });
  showTestFields();

  // --- AI ------------------------------------------------------------------

  const aiPanel = document.querySelector("[data-ai-panel]");
  const aiForm = document.querySelector("[data-ai-form]");
  const aiOutput = document.querySelector("[data-ai-output]");

  aiForm?.addEventListener("submit", async (event) => {
    event.preventDefault();
    const button = aiForm.querySelector("button[type=submit]");
    button.disabled = true;
    aiOutput.replaceChildren(element("p", "text-sm text-muted", "Writing, checking and testing the script… this can take a minute."));
    try {
      const draft = await postJson("/automations/ai", {
        prompt: aiForm.elements.prompt.value,
        source: textarea.value,
        name: nameInput?.value || "",
        kind: form.dataset.kind,
        automation_id: automationId,
      });
      const parts = [];
      if (draft.explanation) parts.push(element("p", "text-sm", draft.explanation));
      const problems = draft.diagnostics.filter((d) => d.severity === "error").length;
      if (problems || draft.load_error) {
        parts.push(element("p", "test-error", "The script still has problems; check them after using it."));
      }
      const preview = element("pre", "code-preview");
      preview.innerHTML = render(draft.source, []);
      parts.push(preview);
      const actions = element("div", "flex flex-wrap gap-2");
      const use = element("button", "btn text-sm", "Use this script");
      use.type = "button";
      use.addEventListener("click", async () => {
        replaceAll(draft.source);
        aiOutput.replaceChildren(element("p", "text-sm", "Replaced the script. Undo (Ctrl+Z) brings the old one back. Save to keep it."));
        await lint();
      });
      const discard = element("button", "btn-quiet text-sm", "Discard");
      discard.type = "button";
      discard.addEventListener("click", () => aiOutput.replaceChildren());
      actions.append(use, discard);
      parts.push(actions);
      aiOutput.replaceChildren(...parts);
    } catch (error) {
      aiOutput.replaceChildren(errorLine(error.message));
    } finally {
      button.disabled = false;
    }
  });

  // --- Toolbar -------------------------------------------------------------

  document.querySelector("[data-action=format]")?.addEventListener("click", format);
  document.querySelector("[data-action=test]")?.addEventListener("click", () => {
    testPanel.scrollIntoView({ block: "nearest", behavior: "smooth" });
    testForm.elements.kind.focus();
  });
  document.querySelector("[data-action=ai]")?.addEventListener("click", () => {
    aiPanel.scrollIntoView({ block: "nearest", behavior: "smooth" });
    (aiForm?.elements.prompt || aiPanel.querySelector("a"))?.focus();
  });

  // Saved versions and other script listings get the same highlighting.
  for (const block of document.querySelectorAll("pre[data-lua]")) {
    block.innerHTML = render(block.textContent, []).replace(/\n $/, "");
  }

  paint();
  lint();
})();
