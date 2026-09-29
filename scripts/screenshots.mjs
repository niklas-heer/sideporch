// Seeds a fresh Sideporch server with a small team's chat and takes the
// README and website screenshots as PNGs.
//
//   sideporch --listen 127.0.0.1:18790 --data "$(mktemp -d)" &
//   npm install --no-save playwright-core
//   CHROME=/path/to/chrome node scripts/screenshots.mjs http://127.0.0.1:18790 /tmp/shots
//
// Then convert them for the README, for example:
//   cwebp -q 82 -resize 1600 0 /tmp/shots/channel.png -o docs/screenshots/channel.webp
//   (phones.png: -q 84 -resize 1100 0)
// and copy the ones the website shows into website/img/.
//
// CHROME defaults to Playwright's headless shell; any Chromium works. Node
// is needed; Bun's fetch breaks Playwright's cookie handling.
import { readFileSync } from "node:fs";
import { chromium } from "playwright-core";

const [base, out] = process.argv.slice(2);
const exe = process.env.CHROME;
const fixtures = new URL("../tests/fixtures/gatus", import.meta.url).pathname;
const browser = await chromium.launch(exe ? { executablePath: exe } : {});
const errors = [];
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function person() {
  const context = await browser.newContext({ timezoneId: "Europe/Berlin", locale: "en-GB" });
  return context;
}
const form = (context, path, fields, extra = {}) =>
  context.request.post(base + path, { form: fields, maxRedirects: 0, ...extra });
const say = (context, channel, body, parent) =>
  form(context, `/c/${channel}/messages`, parent ? { body, parent_id: String(parent) } : { body }, {
    headers: { "x-sideporch-fetch": "1" },
  });
async function lastId(context, channel) {
  const html = await (await context.request.get(`${base}/c/${channel}`)).text();
  const list = html.slice(html.indexOf('id="messages"'));
  const ids = [...list.matchAll(/data-message-id="(\d+)"/g)].map((match) => Number(match[1]));
  return Math.max(...ids);
}
const react = (context, channel, id, emoji) => form(context, `/c/${channel}/m/${id}/reactions`, { emoji });

// Emoji avatars on soft colours.
const avatarPage = await browser.newPage({ viewport: { width: 256, height: 256 } });
async function avatar(emoji, colour) {
  await avatarPage.setContent(
    `<div style="width:256px;height:256px;background:${colour};display:flex;align-items:center;justify-content:center;font-size:150px">${emoji}</div>`,
  );
  return avatarPage.screenshot({ type: "png" });
}
async function profile(context, name, emoji, colour, status) {
  const response = await context.request.post(`${base}/settings/profile`, {
    maxRedirects: 0,
    multipart: {
      display_name: name,
      status_emoji: status[0],
      status_text: status[1],
      bio: "",
      avatar: { name: "me.png", mimeType: "image/png", buffer: await avatar(emoji, colour) },
    },
  });
  if (response.status() !== 303) errors.push(`profile ${name}: ${response.status()}`);
}

// The team.
const ada = await person();
await form(ada, "/setup", { display_name: "Ada", username: "ada", password: "correct horse battery" });
const general = Number((await ada.request.get(base + "/", { maxRedirects: 0 })).headers().location.split("/").pop());
async function invite(name, username) {
  await form(ada, "/invites", {});
  const people = await (await ada.request.get(`${base}/people`)).text();
  const token = people.match(/\/join\/([0-9a-f]+)/)[1];
  const context = await person();
  await form(context, `/join/${token}`, { display_name: name, username, password: "a long password" });
  return context;
}
const grace = await invite("Grace", "grace");
const mo = await invite("Mo", "mo");
const rosa = await invite("Rosa", "rosa");
const linus = await invite("Linus", "linus");
await profile(ada, "Ada", "🦉", "#DDEBE7", [":house:", "Working from the porch"]);
await profile(grace, "Grace", "🌻", "#FCEFC7", [":rocket:", "Shipping v2.3"]);
await profile(mo, "Mo", "🐙", "#E3E7F7", [":headphones:", "Deep work"]);
await profile(rosa, "Rosa", "🍋", "#FBE3DC", ["", ""]);
await profile(linus, "Linus", "🌿", "#E4F0DA", [":coffee:", ""]);

async function channel(context, name, isPrivate = false, topic = "") {
  const fields = isPrivate ? { name, private: "on" } : { name };
  const id = Number((await form(context, "/channels", fields)).headers().location.split("/").pop());
  if (topic) await form(context, `/c/${id}/topic`, { topic });
  return id;
}
await form(ada, `/c/${general}/topic`, { topic: "Everything else. Be kind, write things down." });
const design = await channel(grace, "design", false, "Mockups, feedback, fonts");
const deploys = await channel(linus, "deploys", false, "Alerts, releases and approvals");
const random = await channel(mo, "random");
const plans = await channel(ada, "offsite-plans", true, "Shh");
for (const who of [grace, mo]) {
  const id = await (who === grace ? grace : mo).request.get(`${base}/home`).then((r) => r.text()).then((t) => t.match(/data-me="(\d+)"/)[1]);
  await form(ada, `/c/${plans}/members`, { user_id: id });
}

// #general: a release conversation with a thread, reactions and a poll.
await say(ada, general, "Morning, porch! ☕ Standup notes are in the wiki as usual.");
await say(grace, general, "The **v2.3 release notes** are ready for review 🎉\n\nHighlights: offline mode, faster search, and the new *Scheduled* page. Comments in the thread please 👇");
const notes = await lastId(grace, general);
await say(mo, general, "Looks great. One nit: the changelog lists `--dry-run` twice.", notes);
await say(ada, general, "Fixed, thanks @mo! Also added the migration note for self-hosters.", notes);
await say(rosa, general, "Screenshots are updated too :camera_flash:", notes);
await say(grace, general, "Perfect, merging now :rocket:", notes);
for (const [who, emoji] of [[ada, "tada"], [mo, "tada"], [rosa, "tada"], [linus, "eyes"], [mo, "heart"]]) {
  await react(who, general, notes, emoji);
}
await say(linus, general, "Release checklist:\n\n- [x] Migrations tested on a copy of production\n- [x] Screenshots updated\n- [ ] Blog post\n- [ ] Tag `v2.3.0`");
const checklist = await lastId(linus, general);
await react(grace, general, checklist, "white_check_mark");
// A ranked poll: Pizza ties Ramen on first choices, but Ramen wins once
// the beer garden and taco fans' votes move on.
await form(
  grace,
  `/c/${general}/polls`,
  { question: "Where do we celebrate the release?", options: "Ramen Ichi\nPizza Nonna\nTaco truck\nThe beer garden", kind: "ranked" },
  { headers: { "x-sideporch-fetch": "1" } },
);
const poll = await lastId(grace, general);
for (const [who, ranks] of [
  [ada, { r0: "1", r1: "2" }],
  [grace, { r0: "1", r2: "2" }],
  [mo, { r2: "1", r0: "2" }],
  [rosa, { r1: "1", r0: "2" }],
  [linus, { r1: "1", r2: "2" }],
]) {
  await form(who, `/c/${general}/m/${poll}/rank`, ranks, { headers: { "x-sideporch-fetch": "1" } });
}
await say(rosa, general, "Ramen it is. I'll book a table for Thursday 🍜");

// #design
await say(rosa, design, "New onboarding illustrations, v3. Warmer colours, fewer words.");
await say(grace, design, "Love the porch lantern 😍");
await say(mo, design, "Can we try the headline in *Atkinson Hyperlegible*? It reads better at small sizes.");

// #deploys: an automation with approval buttons, Gatus alerts, and a diagram.
await form(ada, "/automations", {
  name: "Deploy approvals",
  enabled: "on",
  source: `-- Asks for approval before a deploy.
sideporch.on("message", { channel = "deploys", pattern = "^!deploy" }, function(msg)
  local service = msg.text:match("^!deploy%s+(%S+)") or "app"
  sideporch.reply(msg, "Deploy **" .. service .. "** to production?", { buttons = {
    { label = "Approve", value = service, style = "primary" },
    { label = "Cancel", value = "cancel", style = "danger" },
  } })
end)

-- Whoever clicks decides.
sideporch.on("button", function(click)
  if click.value == "cancel" then
    sideporch.update(click.message, "Deploy cancelled by " .. click.user, { buttons = {} })
  else
    sideporch.update(click.message, ":rocket: Deploying **" .. click.value .. "**, approved by " .. click.user, { buttons = {} })
  end
end)

-- Weekly reminder.
sideporch.cron("0 9 * * mon", function()
  sideporch.post("deploys", "New week! Deploy freeze starts Thursday 18:00.")
end)
`,
});
await form(linus, `/c/${deploys}/webhooks`, { name: "Gatus" });
const settings = await (await linus.request.get(`${base}/c/${deploys}/settings`)).text();
const hook = settings.match(/\/hooks\/([0-9a-f]+)/)[1];
const gatus = (file) => JSON.parse(readFileSync(`${fixtures}/${file}`, "utf8").replaceAll("website", "api").replaceAll("Homepage", "API"));
await ada.request.post(`${base}/hooks/${hook}`, { data: gatus("slack-triggered.json") });
await say(mo, deploys, "Looking into it, probably the connection pool again.");
await sleep(300);
await ada.request.post(`${base}/hooks/${hook}`, { data: gatus("slack-resolved.json") });
await say(linus, deploys, "How a change gets to production now:\n\n```mermaid\ngraph LR\n  PR[Pull request] --> CI[CI checks]\n  CI --> S[Staging]\n  S --> A{Approved?}\n  A -- yes --> P[Production]\n  A -- no --> PR\n```");
await say(grace, deploys, "!deploy api v2.3.0");
await sleep(1500);
await say(mo, deploys, "!deploy web v2.3.0");
await sleep(1500);
// Direct messages and a reminder for the sidebar.
const moId = await mo.request.get(`${base}/home`).then((r) => r.text()).then((t) => t.match(/data-me="(\d+)"/)[1]);
const dm = Number((await ada.request.get(`${base}/dm/${moId}`, { maxRedirects: 0 })).headers().location.split("/").pop());
await say(mo, dm, "Got a minute to pair on the search ranking later?");
await say(grace, plans, "Cabin is booked for October 🏡 Don't tell the others yet!");

// The porch opens to the neighbourhood: sign-up, a spammer, a report, and
// someone asking to join.
const community = (registration) => ({
  registration,
  rules: "Be kind. No ads. Keep it about the porch.",
  days_1: "1", visits_1: "1", messages_1: "3",
  days_2: "7", visits_2: "3", messages_2: "20",
  days_3: "30", visits_3: "15", messages_3: "100",
  new_member_per_minute: "6",
});
await form(ada, "/admin/community", community("open"));
const spammer = await person();
await form(spammer, "/signup", { display_name: "Deal Finder", username: "dealfinder", password: "a long password", rules: "agreed", website: "" });
await say(spammer, random, "DM me for free followers and crypto giveaways 💸💸💸");
const spam = await lastId(ada, random);
await form(rosa, `/c/${random}/m/${spam}/report`, { reason: "Spam, and they messaged me too" });
await form(ada, "/admin/community", community("approval"));
const priya = await person();
await form(priya, "/signup", {
  display_name: "Priya",
  username: "priya",
  password: "a long password",
  note: "I live two houses down and run the Saturday garden swap. Would love to join!",
  rules: "agreed",
  website: "",
});

// Screenshots.
async function view(options = {}) {
  const context = await browser.newContext({
    viewport: options.viewport ?? { width: 1360, height: 920 },
    deviceScaleFactor: 2,
    colorScheme: options.dark ? "dark" : "light",
    // A morning somewhere, so timestamps look like a working day.
    timezoneId: "Australia/Sydney",
    locale: "en-GB",
    storageState: await ada.storageState(),
    isMobile: options.mobile ?? false,
    hasTouch: options.mobile ?? false,
  });
  const page = await context.newPage();
  page.on("pageerror", (error) => errors.push(String(error)));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  return page;
}
async function shot(page, name) {
  await page.waitForTimeout(700);
  await page.screenshot({ path: `${out}/${name}.png` });
}

// Approve the api deploy as Ada, through the page.
{
  const page = await view({ viewport: { width: 1360, height: 1040 } });
  await page.goto(`${base}/c/${deploys}`);
  await page.waitForTimeout(500);
  const threads = await page.$$eval("a[data-reply-count].inline-flex", (links) => links.map((link) => link.getAttribute("href")));
  await page.goto(base + threads[0]);
  await page.click("aside button:has-text('Approve')");
  await page.waitForSelector("aside :text('approved by Ada')", { timeout: 5000 }).catch(() => errors.push("approval did not show"));
  await page.close();
}

for (const dark of [false, true]) {
  const page = await view({ dark });
  await page.goto(`${base}/c/${general}/t/${notes}`);
  await page.waitForSelector("[data-poll]");
  await shot(page, dark ? "channel-dark" : "channel");
  await page.close();
}
{
  const page = await view({ viewport: { width: 1360, height: 1040 } });
  await page.goto(`${base}/c/${deploys}`);
  await page.waitForSelector("pre.mermaid[data-processed] svg", { timeout: 15000 }).catch(() => errors.push("no diagram"));
  // Open the thread that still waits for approval.
  const threads = await page.$$eval("a[data-reply-count].inline-flex", (links) => links.map((link) => link.getAttribute("href")));
  await page.goto(base + threads[threads.length - 1]);
  await page.waitForSelector("pre.mermaid[data-processed] svg", { timeout: 15000 }).catch(() => errors.push("no diagram in thread view"));
  await page.evaluate(() => {
    const scroller = document.getElementById("scroller");
    scroller.scrollTop = scroller.scrollHeight;
  });
  await shot(page, "deploys");
  await page.close();
}
{
  const page = await view();
  await page.goto(`${base}/automations`);
  const editor = await page.$$eval("a[href^='/automations/']", (links) => links.map((a) => a.getAttribute("href")).find((href) => /\/automations\/\d+$/.test(href)));
  await page.goto(base + editor);
  await page.waitForTimeout(1200);
  await shot(page, "automation");
  await page.close();
}
{
  const page = await view();
  await page.goto(`${base}/c/${general}`);
  await page.waitForSelector("[data-poll]");
  await page.keyboard.press("Control+k");
  await page.keyboard.type("de");
  await shot(page, "switcher");
  await page.close();
}
{
  // A misspelled search, corrected from the team's own words.
  const page = await view();
  await page.goto(`${base}/search?q=${encodeURIComponent("relase notes")}`);
  await shot(page, "search");
  await page.close();
}
{
  const page = await view({ viewport: { width: 1360, height: 1040 } });
  await page.goto(`${base}/moderation`);
  await shot(page, "moderation");
  await page.close();
}
{
  const page = await view({ viewport: { width: 390, height: 844 }, mobile: true });
  await page.goto(`${base}/c/${general}/t/${notes}`);
  await page.waitForTimeout(500);
  await shot(page, "phone");
  await page.goto(`${base}/home`);
  await shot(page, "phone-home");
  await page.close();
}
{
  // Two phones side by side, framed.
  const page = await browser.newPage({ viewport: { width: 1000, height: 960 }, deviceScaleFactor: 2 });
  const image = (name) => `data:image/png;base64,${readFileSync(`${out}/${name}.png`).toString("base64")}`;
  await page.setContent(`<body style="margin:0;background:transparent">
    <div id="frame" style="display:flex;gap:56px;justify-content:center;padding:48px;background:linear-gradient(135deg,#DDEBE7,#F6F1E4)">
      ${["phone-home", "phone"].map((name) => `<img src="${image(name)}" style="width:390px;border-radius:36px;border:10px solid #1B2F2C;box-shadow:0 24px 48px rgba(27,47,44,.25)">`).join("")}
    </div></body>`);
  await page.locator("#frame").screenshot({ path: `${out}/phones.png` });
  await page.close();
}
console.log(JSON.stringify({ errors }, null, 1));
await browser.close();
