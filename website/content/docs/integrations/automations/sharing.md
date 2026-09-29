+++
title = "Sharing automations"
description = "Export automations with the libraries they need, and import them on another server after a preview."
weight = 6
+++

Automations travel between servers as files. Export the ones you want to share, send the file or post it somewhere, and import it on another server. Importing always shows what the file holds first, and adds everything switched off. New in 0.6.0.

## Exporting

On the Automations page, tick the automations and libraries you want to share and press **Export**. With nothing ticked, **Export** takes all of them. In the editor, **Export** under the script exports that one automation.

{{<shot name="automation-list" alt="The Automations page with two automations ticked for export, the Import and Export buttons, and the libraries below." caption="Tick automations to export just those; the libraries they need come along." />}}

Your browser downloads a file named after the automation, like `deploy-approvals.sideporch.json`, or `automations.sideporch.json` for several. It holds:

- each automation's and library's **name** and **script**,
- every **library** the automations `require`, even when you didn't tick it, including libraries those libraries need,
- the **names of the secrets** each script reads with `sideporch.secret`, so whoever imports it knows what to set up.

It never holds secret values, the automation's saved data, its run log, its webhook URL or its history. That makes a file safe to share, as long as nobody wrote a token straight into a script; keep those in [secrets](@/docs/integrations/automations/data-and-services.md#secrets).

## Importing

On the Automations page, press **Import**. Choose the file, or paste its text, and press **Preview**.

{{<shot name="automation-import" alt="The import preview: a new library, and an automation whose name is taken, with the secret it needs, and a choice to import it as a copy." caption="The preview says what's new, what clashes, and what's missing, before anything is added." />}}

The preview lists every automation and library in the file:

- **New**, or that this server **already has one with this name**, or that it's **already here, unchanged**.
- The **secrets** it reads, marked when this server doesn't have them yet, and the **libraries** it loads, marked when they're neither here nor in the file.
- What the **linter** found, and the script itself under **Read the script**.

Choose what to do with each:

| Choice | What happens |
| --- | --- |
| **Import, switched off** | Adds it. Offered for new ones. |
| **Import as a copy** | Adds it next to the existing one, named like `Weather (imported)`. Offered for automations whose name is taken. |
| **Replace** | Overwrites the existing one's script, and switches it off. Its webhook URL, data and history stay; the old script is one click away in its history. |
| **Skip** | Leaves it out. |

Libraries can only be imported, replaced or skipped: automations find them by name, so a copy under another name wouldn't be used.

Press **Import**. The Automations page says how many were added.

## After importing

Imported automations are switched off, because they can post, react and call other services with this server's secrets. For each one:

1. Open it and read the script. Change channel names to yours.
2. Add the secrets it needs under **Automations → Secrets**.
3. Run a test.
4. Tick **Run this automation** and save.

A library needs no switch; it's used as soon as a running automation requires it.

Agents connected over [MCP](@/docs/integrations/automations/ai-and-mcp.md) can do the same with `export_automations`, `preview_import` and `import_automations`, and imports from them start switched off too.

## The file format

The file is JSON, readable in any text editor:

```json
{
  "format": "sideporch-automations",
  "version": 1,
  "sideporch": "0.6.0",
  "items": [
    {
      "kind": "library",
      "name": "weather",
      "source": "local weather = {}\n…\nreturn weather\n",
      "secrets": [],
      "requires": []
    },
    {
      "kind": "automation",
      "name": "Weather",
      "source": "local weather = require(\"weather\")\n…",
      "secrets": [],
      "requires": ["weather"]
    }
  ]
}
```

- `format` and `version` identify the file. A server refuses other formats, and files from a newer version it doesn't understand yet, with a note to update Sideporch.
- `sideporch` is the version that exported it.
- `kind` is `automation` or `library`.
- `secrets` and `requires` are informational: on import, Sideporch reads them from the script again rather than trusting the file.
- Optional `description`, `author`, `license` and `homepage` fields, on the file or on an item, describe what's shared. Sideporch shows descriptions in the preview.

Servers ignore fields they don't know, so files can carry more without breaking older servers. A file holds at most 100 scripts and 2 MB.
