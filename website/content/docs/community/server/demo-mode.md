+++
title = "Demo mode"
description = "Run a server anyone can try, which starts over every day and keeps only what you chose."
weight = 5
+++

Demo mode is for a server that anyone may join to try Sideporch, like [the live demo](https://sideporch-demo.fly.dev). Every day at a set hour, the server forgets everyone and everything except what you keep, so spam, test messages and anything shady are gone by the next morning.

Turn it on under **Admin → Demo**, pick the hour (in UTC), and tick the channels to keep. New in 0.6.0.

## What a reset keeps

- The **channels you ticked**, public or private, with their messages from people who stay. Keep an announcement channel where only admins post, to greet visitors and tell them what's new, and your own private channels.
- **Admins and everyone with a role.** Give a role to the people who help run the demo, and they stay too.
- Automations and libraries, custom emoji, the GIF library, [bans](@/docs/community/people/moderation.md#bans), and every setting.

## What a reset removes

- Everyone else, with everything they wrote, reacted and uploaded, even in kept channels.
- Every channel you didn't tick, and every direct message.
- Invites, sign-up requests and reports, and files nobody uses anymore.

If no public channel is left, a new `#general` is created, so visitors always land somewhere.

The sidebar and the sign-in page tell everyone when the server starts over. **Reset now** under **Admin → Demo** runs a reset at once, the same way the daily one does.

## Setting up a demo

1. Create a channel for announcements, and under its settings let only managers post.
2. Set **sign-up** to **Anyone can sign up** under **Admin → Community**. The [proof of work and limits per address](@/docs/community/people/sign-up-and-trust.md#how-people-join) keep bots out.
3. Behind a proxy, set [`--client-ip-header`](@/docs/get-started/run-on-a-server.md#options), so limits and bans see visitors' addresses.
4. Turn on demo mode, tick the channels to keep, and press **Reset now** to start clean.

{% <note kind="warning"> %}
A reset deletes for good. Back up first if something on the server matters, and keep your own work in kept channels.
{% </note> %}
