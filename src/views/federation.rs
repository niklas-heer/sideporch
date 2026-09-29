//! Admin → Connections: this server, requests to connect, connected servers.

use maud::{Markup, html};

use super::{Shell, admin::tabs, form_error, panel_page};
use crate::federation::{
    data::{Instance, Status},
    keys::fingerprint,
};

pub struct ConnectionsView<'a> {
    /// This server's public URL, without which it can't connect.
    pub url: Option<&'a str>,
    pub name: Option<&'a str>,
    pub fingerprint: &'a str,
    pub instances: &'a [Instance],
    /// Channels other servers offered to share.
    pub offers: &'a [crate::federation::data::Offer],
    /// Servers with undelivered events: the server, how many, and why the
    /// last try failed.
    pub backlogs: &'a [(i64, i64, Option<String>)],
    pub error: Option<&'a str>,
}

fn key_of(instance: &Instance) -> Markup {
    html! {
        @if let Some(key) = &instance.public_key {
            p class="text-sm text-muted dark:text-haint" {
                "Key fingerprint " code data-fingerprint { (fingerprint(key)) }
            }
        }
    }
}

fn server_name(instance: &Instance) -> Markup {
    html! {
        strong { (if instance.name.is_empty() { instance.handle.as_str() } else { instance.name.as_str() }) }
        " "
        a href=(instance.url) class="text-sm text-muted underline dark:text-haint" { (instance.handle) }
    }
}

fn with_status<'a>(instances: &'a [Instance], statuses: &[Status]) -> Vec<&'a Instance> {
    instances
        .iter()
        .filter(|instance| statuses.contains(&instance.status))
        .collect()
}

pub fn connections_page(shell: &Shell<'_>, view: &ConnectionsView<'_>) -> Markup {
    let pending = with_status(view.instances, &[Status::Pending]);
    let ended = with_status(view.instances, &[Status::Declined, Status::Disconnected]);
    panel_page(
        "Connections",
        shell,
        &html! { "Connections" },
        &html! {
            (tabs("/admin/connections"))
            (form_error(view.error))
            (this_server(view))
            @if !pending.is_empty() {
                (requests(&pending))
            }
            @if !view.offers.is_empty() {
                (offers(view))
            }
            (connected_servers(view))
            @if !ended.is_empty() {
                (super::section("Earlier", "", &html! {
                    ul {
                        @for instance in ended {
                            li class="py-1 text-sm" data-status=(instance.status.key()) {
                                (server_name(instance)) " · "
                                @if instance.status == Status::Declined { "Declined" } @else { "Disconnected" }
                            }
                        }
                    }
                }))
            }
        },
    )
}

/// This server's address, name and key, and asking another to connect.
fn this_server(view: &ConnectionsView<'_>) -> Markup {
    html! {
        (super::section("This server", "Other Sideporch servers see this name and key. Compare the key's fingerprint with the other admin, for example on a call, before you accept each other.", &html! {
            @if let Some(url) = view.url {
                p class="mb-2" { code { (url) } }
                p class="mb-4 text-sm" { "Key fingerprint " code data-fingerprint { (view.fingerprint) } }
                form method="post" action="/admin/connections/name" class="flex flex-wrap items-end gap-2" {
                    label class="min-w-0 flex-1" {
                        span class="field-label" { "Name" }
                        input type="text" name="name" maxlength="80" class="field" value=[view.name] placeholder="The garden club";
                    }
                    button type="submit" class="btn-quiet" { "Save" }
                }
            } @else {
                p { "To connect with other servers, Sideporch needs its public address: start it with " code { "--public-url https://chat.example.org" } "." }
            }
        }))
        @if view.url.is_some() {
            (super::section("Connect to another Sideporch", "Their admin gets your request and decides. Once they accept, you can share channels with them, and people can write to each other directly.", &html! {
                form method="post" action="/admin/connections" class="max-w-xl space-y-3" {
                    label class="block" {
                        span class="field-label" { "Their address" }
                        input type="text" name="url" required class="field" placeholder="https://chat.example.org";
                    }
                    label class="block" {
                        span class="field-label" { "A note for their admin" }
                        textarea name="note" rows="2" maxlength="500" class="field" placeholder="Hi, it's Ada from the garden club. Shall we share #swap?" {}
                    }
                    button type="submit" class="btn" { "Ask to connect" }
                }
            }))
        }
    }
}

/// Servers asking to connect, for an admin to accept or decline.
fn requests(pending: &[&Instance]) -> Markup {
    html! {
        (super::section("Asking to connect", "", &html! {
            ul {
                @for instance in pending {
                    li class="border-b border-line py-3 last:border-b-0 dark:border-night-line" data-status=(instance.status.key()) {
                        p { (server_name(instance)) " wants to connect." }
                        @if !instance.note.is_empty() { p class="mt-1 italic" { "“" (instance.note) "”" } }
                        (key_of(instance))
                        div class="mt-2 flex gap-2" {
                            form method="post" action={ "/admin/connections/" (instance.id) "/accept" } {
                                button type="submit" class="btn" { "Accept" }
                            }
                            form method="post" action={ "/admin/connections/" (instance.id) "/decline" } {
                                button type="submit" class="btn-quiet" { "Decline" }
                            }
                        }
                    }
                }
            }
        }))
    }
}

/// Channels other servers offered, to take or decline.
fn offers(view: &ConnectionsView<'_>) -> Markup {
    html! {
        (super::section("Channels shared with this server", "Taking one makes a copy here. Everything said in it reaches the other server too.", &html! {
            ul {
                @for offer in view.offers {
                    li class="border-b border-line py-3 last:border-b-0 dark:border-night-line" {
                        p {
                            strong { "#" (offer.name) }
                            @if offer.private { " · private" }
                            " from "
                            @match view.instances.iter().find(|instance| instance.id == offer.instance_id) {
                                Some(instance) => (server_name(instance)),
                                None => "another server",
                            }
                        }
                        @if !offer.topic.is_empty() { p class="text-sm text-muted dark:text-haint" { (offer.topic) } }
                        div class="mt-2 flex gap-2" {
                            form method="post" action={ "/admin/connections/offers/" (offer.id) "/take" } {
                                button type="submit" class="btn" { "Take it" }
                            }
                            form method="post" action={ "/admin/connections/offers/" (offer.id) "/decline" } {
                                button type="submit" class="btn-quiet" { "Decline" }
                            }
                        }
                    }
                }
            }
        }))
    }
}

/// Connected servers, and those we asked that haven't answered yet.
fn connected_servers(view: &ConnectionsView<'_>) -> Markup {
    let connected = with_status(view.instances, &[Status::Connected]);
    let waiting = with_status(view.instances, &[Status::Requested]);
    html! {
        (super::section("Connected servers", "", &html! {
            @if connected.is_empty() && waiting.is_empty() {
                p class="text-muted dark:text-haint" { "None yet." }
            }
            ul {
                @for instance in connected {
                    li class="border-b border-line py-3 last:border-b-0 dark:border-night-line" data-status=(instance.status.key()) {
                        p { (server_name(instance)) " · " span class="text-sm font-semibold text-floor-3 dark:text-haint" { "Connected" } }
                        (key_of(instance))
                        @if let Some((_, waiting, error)) = view.backlogs.iter().find(|(id, _, _)| *id == instance.id) {
                            p class="mt-1 text-sm text-red-700 dark:text-red-300" data-backlog=(waiting) {
                                (waiting) @if *waiting == 1 { " message is" } @else { " messages are" } " waiting to reach them"
                                @if let Some(error) = error { ": " (error) }
                                ". Sideporch keeps trying."
                            }
                        }
                        div class="mt-2 flex flex-wrap items-center gap-3" {
                            form method="post" action={ "/admin/connections/" (instance.id) "/direct" } class="flex items-center gap-2" {
                                label class="flex items-center gap-2 text-sm" {
                                    input type="checkbox" name="allow" value="on" checked[instance.allow_direct];
                                    "People there may write to people here directly"
                                }
                                button type="submit" class="btn-quiet text-sm" { "Save" }
                            }
                            form method="post" action={ "/admin/connections/" (instance.id) "/disconnect" } {
                                button type="submit" class="btn-quiet text-sm" { "Disconnect" }
                            }
                        }
                    }
                }
                @for instance in waiting {
                    li class="border-b border-line py-3 last:border-b-0 dark:border-night-line" data-status=(instance.status.key()) {
                        p { (server_name(instance)) " · " span class="text-sm text-muted dark:text-haint" { "Waiting for them to accept" } }
                        (key_of(instance))
                        form method="post" action={ "/admin/connections/" (instance.id) "/disconnect" } class="mt-2" {
                            button type="submit" class="btn-quiet text-sm" { "Withdraw" }
                        }
                    }
                }
            }
        }))
    }
}

/// People found on connected servers, to write to.
pub fn elsewhere_page(
    shell: &Shell<'_>,
    query: &str,
    found: &[crate::routes::federation::Found],
    problems: &[String],
) -> Markup {
    panel_page(
        "People elsewhere",
        shell,
        &html! { "People on other servers" },
        &html! {
            p class="mb-4 max-w-xl text-muted dark:text-haint" {
                "Find people on the Sideporch servers this one is connected to, and write to them. Your conversation stays on both servers."
            }
            form method="get" action="/people/elsewhere" class="mb-6 flex max-w-xl gap-2" {
                input type="search" name="q" value=(query) required minlength="2" class="field flex-1" placeholder="Name or username" aria-label="Name or username";
                button type="submit" class="btn" { "Search" }
            }
            @for problem in problems {
                p class="mb-2 text-sm text-red-700 dark:text-red-300" { (problem) }
            }
            @if found.is_empty() && query.trim().chars().count() >= 2 {
                p class="text-muted dark:text-haint" { "Nobody by that name on the connected servers." }
            }
            ul class="max-w-xl" {
                @for person in found {
                    li class="flex items-center gap-3 border-b border-line py-2 last:border-b-0 dark:border-night-line" {
                        span class="min-w-0 flex-1" {
                            span class="block font-semibold" { (person.display_name) }
                            span class="block text-sm text-muted dark:text-haint" { "@" (person.username) "@" (person.server.handle) }
                        }
                        form method="post" action="/people/elsewhere/message" {
                            input type="hidden" name="server" value=(person.server.id);
                            input type="hidden" name="id" value=(person.id);
                            button type="submit" class="btn-quiet" { "Message" }
                        }
                    }
                }
            }
        },
    )
}
