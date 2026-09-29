+++
title = "Sign-up, trust levels and permissions"
description = "Decide who can join, and let newcomers earn uploads, links and more as they take part."
weight = 2
aliases = ["/docs/community/sign-up-and-trust/"]
+++

Sideporch works for a team behind invite links, and just as well for a community anyone can join. The settings are under **Admin → Community** and **Admin → Permissions**.

## How people join

| Sign-up | Who gets in |
| --- | --- |
| **With an invite link** (the default) | People with a link from someone allowed to invite. |
| **Ask to join** | Anyone can ask to join, with a note; an admin or moderator lets them in under **Moderation**. |
| **Anyone can sign up** | Anyone who finds the server. |

With the last two, you can also set:

- **Rules** people agree to when signing up, written in Markdown.
- How many messages a minute new members at level 0 may send; 6 by default. With **Ask to join**, people a moderator lets in start at level 1, so this matters most when anyone can sign up.

Bots are kept out without anyone having to solve a puzzle:

- The sign-up form makes the browser do a small **proof of work**: it computes hashes until it finds one that starts with enough zeros, which takes a moment. That's nothing for a person, and a real cost for a script creating accounts by the thousand. No outside CAPTCHA service is involved, but signing up needs JavaScript.
- At most **3 accounts an hour from one address**, and 30 an hour across the server.
- A hidden trap field catches simple bots that fill in every field.

Behind a reverse proxy, set [`--client-ip-header`](@/docs/get-started/run-on-a-server.md#options) so the limit per address sees visitors' addresses.

## Trust levels

Trust levels keep newcomers from doing harm while they're new, without making everyone ask an admin for everything.

People who sign up on their own start at level **0**; people who join with an invite, or whom a moderator lets in, at level **1**. They move up by themselves as they stay and take part:

| Level | Reached after | Days visited | Messages sent |
| --- | --- | --- | --- |
| 1 | 1 day | 1 | 3 |
| 2 | 7 days | 3 | 20 |
| 3 | 30 days | 15 | 100 |
| 4 | only set by an admin | | |

Admins change the requirements under **Admin → Community**. Levels never go down on their own. On someone's profile, an admin can set their level and **keep it there**, so it neither rises nor falls.

People see their own level on their profile, and what the next one still takes ("Level 2 after 3 more days, 12 more messages"). When they reach a level, a note in the sidebar says so, and what they can do now.

## Permissions

Each permission asks for a trust level, or is left to roles. Admins may always do everything. The defaults:

| Permission | Default |
| --- | --- |
| Create polls | Level 0 |
| Upload files and images | Level 1 |
| Post links | Level 1 |
| Notify everyone with `@channel` and `@here` | Level 1 |
| Start direct messages | Level 1 |
| Create public channels | Level 1 |
| Create private channels | Level 1 |
| Add custom emoji | Level 1 |
| See statistics | Level 1 |
| Invite people | Roles only |
| Moderate | Roles only |

Change them under **Admin → Permissions**. To turn a feature off, such as uploads, set it to **Roles only** and give no role that permission.

Someone who may not do something yet is told so, and that they'll earn more as they take part.

## Roles

Roles give permissions to people regardless of their level, such as "Moderators" or "Designers". Create them under **Admin → Permissions**, pick their permissions, and give them to people from their profiles. A role's name shows on the profiles of the people who have it.

Tick **Show as a badge** and pick a color, and the role also shows next to its people's names in chat and on their hover cards, so everyone can tell who the moderators are. Admins always have an **Admin** badge. Someone with several badge roles shows the first in alphabetical order next to their name, and all of them on their profile.
