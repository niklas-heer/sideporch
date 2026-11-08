+++
title = "Phones and notifications"
description = "Install Sideporch like an app on iPhone and Android, and turn on push notifications."
weight = 6
+++

Sideporch works in any mobile browser, and installs like an app: its own icon and window, the number of unread conversations on the icon, and on Android a place in the share sheet, so links and text from other apps go straight into a conversation. When the connection drops, it says so instead of showing an error page.

{{<shot name="phones" alt="Sideporch on two phones: the channel list, and a thread." caption="The channel list and a thread on a phone." />}}

## Turn on notifications

Notifications need your server on HTTPS (see [Run it on a server](@/docs/get-started/run-on-a-server.md#put-it-behind-https)). Then:

- **iPhone and iPad** (iOS 16.4 or newer): open Sideporch in Safari, tap **Share**, then **Add to Home Screen**. Open it from the home screen and tap **Turn on** in the hint at the bottom of the sidebar, or **Notifications** in your account menu (your name at the bottom of the sidebar). iOS only lets installed web apps send notifications, so this step is needed.
- **Android**: open Sideporch in Chrome or Firefox and tap **Turn on**, or **Notifications** in your account menu. Installing it (**Install** in the hint, or *Add to Home screen*) is optional but gives it its own window.
- **Computers**: **Notifications** in your account menu works in every current browser.

Each device turns notifications on separately.

## What notifies you

Direct messages, mentions and replies in your threads, but not the conversation you're looking at. [Mute a channel](@/docs/using/keeping-up.md#notifications) to keep it quiet.

## Pauses and quiet hours

Account menu → **Pauses and quiet hours** (under your name, in the sidebar) holds two controls. Both only stop push notifications on your devices. Mentions still appear in **Activity**, unread markers still update, and nothing is held back to send later.

- **Pause** stops notifications for an hour, for eight hours, or until 08:00 tomorrow, in your time zone. While it runs, the account menu says **Paused until 14:30** (or a later day and time). **Resume** ends it early.
- **Quiet hours** repeat every week: pick a start and end time and the days. They run on your own clock, in the time zone your browser reports. A window that crosses midnight counts for the day it starts on, so Friday 22:00 to 07:00 covers Friday night and the early hours of Saturday, but not Saturday night.

Each device still needs notifications turned on, and pauses and quiet hours apply to all of them.

Sideporch sends notifications itself, through the browsers' push services, so no app store account is involved and your messages never pass through a third-party notification service in readable form: push messages are encrypted for your device. Your server only needs to reach the internet.
