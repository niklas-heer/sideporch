+++
title = "Phones and notifications"
description = "Install Sideporch like an app on iPhone and Android, and turn on push notifications."
weight = 5
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

Sideporch sends notifications itself, through the browsers' push services, so no app store account is involved and your messages never pass through a third-party notification service in readable form: push messages are encrypted for your device. Your server only needs to reach the internet.
