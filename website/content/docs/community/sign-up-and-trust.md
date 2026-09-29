+++
title = "Sign-up, trust levels and permissions"
description = "Decide who can join, and let newcomers earn uploads, links and more as they take part."
weight = 2
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
- How many messages a minute new members (level 0) may send; 6 by default.

Sign-ups are limited to 30 an hour across the server, so a script can't create accounts by the thousand, and a hidden trap field catches simple bots.

## Trust levels

Trust levels keep newcomers from doing harm while they're new, without making everyone ask an admin for everything.

People who sign up on their own start at level **0**; people who join with an invite at level **1**. They move up by themselves as they stay and take part:

| Level | Reached after | Days visited | Messages sent |
| --- | --- | --- | --- |
| 1 | 1 day | 1 | 3 |
| 2 | 7 days | 3 | 20 |
| 3 | 30 days | 15 | 100 |
| 4 | only set by an admin | | |

Admins change the requirements under **Admin → Community**. Levels never go down on their own. On someone's profile, an admin can set their level and **keep it there**, so it neither rises nor falls.

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
| Invite people | Roles only |
| Moderate | Roles only |

Change them under **Admin → Permissions**. To turn a feature off, such as uploads, set it to **Roles only** and give no role that permission.

Someone who may not do something yet is told so, and that they'll earn more as they take part.

## Roles

Roles give permissions to people regardless of their level, such as "Moderators" or "Designers". Create them under **Admin → Permissions**, pick their permissions, and give them to people from their profiles. A role's name shows on the profiles of the people who have it.
