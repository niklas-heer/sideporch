+++
title = "Moderation"
description = "Handle reports, approve people who asked to join, time people out, and ban people, addresses and email domains."
weight = 3
aliases = ["/docs/community/moderation/"]
+++

Admins and people with the **Moderate** permission (usually through a "Moderators" [role](@/docs/community/people/sign-up-and-trust.md#roles)) look after the community from **Moderation** in the sidebar.

{{<shot name="moderation" alt="The moderation page: someone asking to join with a note, and a reported spam message with buttons to delete it or dismiss the report." caption="Someone asking to join, and a reported message." />}}

## Reports

Anyone can **report** a message from its menu, with a reason. Moderators see the reports under **Moderation**, and either **delete** the message or **dismiss** the report.

## People who asked to join

When [sign-up](@/docs/community/people/sign-up-and-trust.md#how-people-join) is set to **Ask to join**, their requests wait under **Moderation** with the note they wrote. **Let them in**, or **Decline**.

## Time-outs

A time-out stops someone from posting or reacting for **1 hour**, **1 day** or **1 week**, while they can still read. Give one from their profile; **Moderation** lists who is timed out, and the profile ends a time-out early.

## Bans

A time-out pauses someone; a ban keeps them out. Moderators ban from the person's profile, which shows the **addresses** they used in the last 30 days, and other accounts that used the same ones. **Ban** there:

- deactivates the account and signs it out everywhere,
- optionally bans those addresses, so nobody from them can use the server at all, not even to sign up again,
- optionally bans their email address, so it can't be used for a new account,
- optionally **removes all their messages, reactions and votes**, for spammers.

Under **Moderation → Bans**, moderators also ban addresses directly (`203.0.113.7`), ranges (`203.0.113.0/24`, `2001:db8::/48`), email addresses, and whole email domains (`@spam.example`, which covers its subdomains too). Bans last a day, a week, 30 days, or until someone lifts them. Nobody can ban an admin, or an address range that includes their own address.

{% <note kind="warning"> %}
Many people can share one address: everyone in an office, a school, or on the same mobile network. Check who else used an address, shown on the profile, before banning it, and prefer short bans for addresses.
{% </note> %}

Addresses are personal data. Sideporch notes each account's address at most once a day while it's signed in, keeps them for 30 days, and shows them only to admins and moderators. Behind a reverse proxy, set [`--client-ip-header`](@/docs/get-started/run-on-a-server.md#options), or every visitor looks like the proxy.

## Also moderators' work

- Delete anyone's message.
- New members can only send a few messages a minute (6 by default), which slows down spam while a moderator steps in.
- Nobody but admins can send the same message (of 12 characters or more) a third time within ten minutes, which stops copy-and-paste spam across channels.
- When two different people report messages of someone at trust level 0, that person is timed out for a day, so a spammer stops even before a moderator is around. The reports wait under **Moderation** as usual; end the time-out from their profile if it was a mistake.
