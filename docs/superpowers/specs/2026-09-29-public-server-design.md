# Running Sideporch in public, and a live demo

Agreed in conversation on 2026-09-29. Ships in the same release as typing, statistics and sharing automations.

## Intent

Sideporch should be safe to run as an open, public server, and there should be one: a live demo on sideporch.app where anyone can join and look around, reset every night so nothing shady stays. The website should explain from a newcomer's point of view why Sideporch is worth caring about.

## 1. Knowing who connects

- The client's address comes from the TCP peer, or from a header a trusted proxy sets: `--client-ip-header` (`SIDEPORCH_CLIENT_IP_HEADER`), such as `Fly-Client-IP` or `X-Forwarded-For` (its first entry). Without the setting, headers are ignored, so nobody can fake an address.
- Addresses of sign-ins and sign-ups (and live connections, at most once a day) are kept per account for 30 days, then deleted. Admins and moderators see them on the person's profile. Documented as personal data.

## 2. Sign-in limits

- Failed password sign-ins: at most 10 per 15 minutes per address and per account; then refused with how long to wait. Counted in memory.
- Sign-ups: at most 3 per hour per address, next to the existing 30 per hour overall.

## 3. Bans

- A ban names an **address or range** (`203.0.113.7`, `203.0.113.0/24`, `2001:db8::/48`), an **email address**, or an **email domain** (`@spam.example`), with a reason and an end (1 day, 1 week, 30 days, never).
- Banned addresses can't use the server at all (a short page says so). Banned emails and domains can't sign up or be added to accounts.
- **Ban** on a profile deactivates the account, signs it out everywhere, and optionally bans its recent addresses and its email, and removes all its messages and reactions.
- Admins and people who may **Moderate** ban; nobody can ban an admin. Moderation lists bans and lifts them.

## 4. Bots and spam

- Open sign-up and ask-to-join forms carry a proof-of-work challenge: the browser finds a nonce whose SHA-256 has enough leading zero bits (about a second of work), the server checks it and accepts each challenge once. No third-party CAPTCHA. Signing up needs JavaScript.
- The same message posted three times within ten minutes by one person is refused the third time.
- When two different people report messages of someone at level 0, that person is timed out for a day until a moderator looks.

## 5. Who is who

- Hovering a person's name or picture in chat shows a card: picture, name, username, status, badges, trust level, local time, when they joined, and buttons to message them and open their profile. Keyboard: focus shows it too.
- Badges: **Admin** for admins, and roles an admin marks **Show as a badge**, with a color. Messages show the person's first badge next to their name.
- Leveling up: people see their progress to the next trust level on their profile, and after reaching one, a note says what they can do now.

## 6. Demo mode

- Admin → Demo: switched off by default. When on, the server resets every day at a set time (in the server's automation time zone) and shows a notice saying when.
- Channels marked **Keep during resets** (in channel settings) keep their messages. A reset keeps admins, people with a role, automations, custom emoji and settings, and deletes everyone else, every other channel, direct messages, files nobody uses anymore, reports and sign-up requests. **Reset now** runs it at once.

## 7. The live demo

- `demo.sideporch.app` on Fly.io: one machine, a volume for `/data`, the release image, `SIDEPORCH_CLIENT_IP_HEADER=Fly-Client-IP`, setup locked to a setup link. Open sign-up, demo mode on. `deploy/fly/` holds the configuration and a short README.

## 8. The website

- Landing page: who it's for and why (your own chat, no per-seat bill, history that stays), a short comparison with Slack, Discord and Mattermost, a **Try the live demo** button, and an FAQ.
- Docs home: entry points by role: people who chat, people who run a server, people evaluating, people automating.

## Order

Addresses → sign-in limits → bans → bots and spam → who is who → demo mode → website → release → deploy the demo.
