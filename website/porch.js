// The only moving part of the page: the conversation in the hero arrives
// once, message by message. Without this script, or with reduced motion,
// it is simply all there. Also copies the install command.
"use strict";

(() => {
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

  for (const button of document.querySelectorAll("button[data-copy]")) {
    button.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(button.dataset.copy);
        button.textContent = "Copied";
      } catch {
        button.textContent = "Select it";
      }
      setTimeout(() => (button.textContent = "Copy"), 2000);
    });
  }
})();
