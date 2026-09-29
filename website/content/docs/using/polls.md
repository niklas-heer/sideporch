+++
title = "Polls"
description = "Ask everyone to pick one option, pick several, or rank them to find what most people can live with."
weight = 2
+++

A poll is a message with a question and 2 to 10 options. Start one from the chart button next to the message box, or with a command.

## Three kinds of polls

| Kind | Command | People… | Good for |
| --- | --- | --- | --- |
| **Pick one** | `/poll Where do we eat? \| Pizza \| Tacos` | choose one option | quick decisions |
| **Pick several** | `/poll multiple Which days work? \| Mon \| Tue \| Wed` | choose every option that works for them | finding a day that suits everyone |
| **Ranked** | `/poll ranked Where do we celebrate? \| Ramen \| Pizza \| Tacos` | rank the options, first choice first | finding the option most people can live with |

Options are separated by `|`. You can also quote each part: `/poll "Where do we eat?" "Pizza" "Tacos"`.

People can change their vote at any time. Whoever started the poll, or a moderator, can end it with **End the poll**; after that, the result stays as it is.

## How ranked polls are counted

Ranked polls are counted by **instant runoff**, so a divided vote doesn't hand the win to an option most people dislike:

1. Everyone's first choice is counted.
2. If one option has more than half of the votes, it wins.
3. Otherwise the option with the fewest votes drops out, and its voters' votes move to their next choice still in the running.
4. That repeats until one option has a majority.

The poll shows who leads, which round it's in, and **how the votes moved**, round by round. People may rank as few or as many options as they like.

{{<shot name="channel" alt="A ranked poll in a channel: Ramen Ichi leads with 3 of 5 votes after 3 rounds, Taco truck was out in round 2 and the beer garden in round 1." caption="Ramen wins once the beer garden and taco fans' votes move to their next choice." dark={true} />}}
