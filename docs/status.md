# `modpack status --json`

`modpack status` reports the state of a pack on this machine: the socle version, the pack's Modrinth instance, the variant loaded there, drift, files of the instance the manifest does not know, how `mods/`, `resourcepacks/` and their local directories compare to the manifest, and the local entries with their approval. It reads files only: it writes nothing and makes no network call.

With `--json`, the report is one JSON object on stdout. This format is the contract read by the socle's return skill (`modpack-return`). Logs go to stderr and never mix with it.

## Exit code

- `0`: the report was produced, whatever it contains. Findings (drift, unknown files, stale manifest) do not change the exit code.
- `2`: not inside a modpack repository, an invalid repository file, or a repository whose socle is newer than the CLI (it does not know that format; nothing is printed on stdout).
- `1`: an unexpected failure, such as an unreadable file.

## Fields

| Field | Meaning |
|---|---|
| `socle.repository` | Socle version of the repository (`.modpack/socle.toml`). |
| `socle.cli` | Version of the CLI. |
| `socle.gap` | `equal`, or `older` when the repository's socle is older than the CLI: `modpack update` brings it up to date. |
| `instance` | The pack's instance on this machine, or `null` when there is none (no entry in the machine's `instances.toml`, or its directory is gone): `modpack setup` creates it. |
| `instance.path` | Absolute path of the instance directory. Its `logs/` and `crash-reports/` are there. |
| `instance.variant` | Variant loaded last, or `null` before the first `modpack variant load`. |
| `instance.drift` | Files the CLI wrote into the instance that changed or were deleted since, relative to the instance, with `/` separators. A file already as loading the variant again would leave it is not drift: same content as the repository's, or deleted where the variant writes nothing. A deleted jar or resource pack counts only once the variant no longer has its entry, under any file name, and never when the app disabled it. When the repository has no such variant, every change counts. |
| `instance.disabled` | Files of `drift` that the Modrinth App disabled: renamed `<file>.disabled`, content unchanged. `modpack variant load <variant> --force` enables them again. |
| `instance.unknown_files` | Jars of the instance's `mods/` and zips of its `resourcepacks/` whose sha1 and sha512 match no manifest entry, whatever their file name, with their hashes so they can be looked up on the platforms. The files of `outdated_files` are not listed. |
| `instance.outdated_files` | Files the CLI wrote into the instance's `mods/` or `resourcepacks/` at an earlier load, unchanged since, that no manifest entry has any more: the entry was removed or updated, or a local entry was rebuilt. `modpack variant load` replaces them. `local_entry` is the key of the local entry whose file has the same name, the file then being its previous build, or `null`. |
| `missing_files` | Manifest entries whose file is absent from the repository, or replaced by an older file `modpack install` downloaded and nobody changed since: `modpack install` downloads them. A local entry listed here lost its file from `mods-local/` or `resourcepacks-local/`: git restores it. |
| `manifest.stale` | `true` when `new_files` or `changed` is not empty: `modpack manifest` regenerates the manifest. |
| `manifest.new_files` | Files of `mods/`, `resourcepacks/` or their local directories that no entry lists, except the ones `modpack install` downloaded and nobody changed since: those are removed by the next `modpack install`, never added back. |
| `manifest.changed` | Entries whose file is in the repository with other hashes than the manifest's. |
| `local_entries` | The local mods and resource packs: files tracked by git in `mods-local/` and `resourcepacks-local/` (see [local mods](local-mods.md)). `approved` is `true` when the approval recorded in the manifest matches the file; the publish workflow refuses to publish while one is `false`, and `modpack approve <key>` approves it. |

Only hashes are compared: the report never asks a platform about a file.

A file listed in `missing_files` does not make the manifest stale: a missing file means "not installed on this machine", never "removed from the pack", and `modpack manifest` keeps its entry. It is the usual case in a fresh clone, since git holds only the local files. An entry leaves the pack only through `modpack remove <key>`, which also takes its key out of the variants.

Lists are sorted. Paths of files are relative (`mods/<name>`, `mods-local/<name>`, `resourcepacks/<name>`, `resourcepacks-local/<name>`), to the instance for `unknown_files`, to the repository otherwise.

## Schema

JSON Schema (draft 2020-12). It describes this version exactly. A later version of the CLI may add fields, so a reader ignores the fields it does not know.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "modpack status",
  "type": "object",
  "additionalProperties": false,
  "required": ["socle", "instance", "missing_files", "manifest", "local_entries"],
  "properties": {
    "socle": {
      "type": "object",
      "additionalProperties": false,
      "required": ["repository", "cli", "gap"],
      "properties": {
        "repository": { "type": "string" },
        "cli": { "type": "string" },
        "gap": { "enum": ["equal", "older"] }
      }
    },
    "instance": {
      "oneOf": [
        { "type": "null" },
        {
          "type": "object",
          "additionalProperties": false,
          "required": ["path", "variant", "drift", "disabled", "unknown_files", "outdated_files"],
          "properties": {
            "path": { "type": "string" },
            "variant": { "type": ["string", "null"] },
            "drift": { "type": "array", "items": { "type": "string" } },
            "disabled": { "type": "array", "items": { "type": "string" } },
            "unknown_files": {
              "type": "array",
              "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["path", "sha1", "sha512"],
                "properties": {
                  "path": { "type": "string" },
                  "sha1": { "type": "string", "pattern": "^[0-9a-f]{40}$" },
                  "sha512": { "type": "string", "pattern": "^[0-9a-f]{128}$" }
                }
              }
            },
            "outdated_files": {
              "type": "array",
              "items": {
                "type": "object",
                "additionalProperties": false,
                "required": ["path", "local_entry"],
                "properties": {
                  "path": { "type": "string" },
                  "local_entry": { "type": ["string", "null"] }
                }
              }
            }
          }
        }
      ]
    },
    "missing_files": { "type": "array", "items": { "$ref": "#/$defs/entryFile" } },
    "manifest": {
      "type": "object",
      "additionalProperties": false,
      "required": ["stale", "new_files", "changed"],
      "properties": {
        "stale": { "type": "boolean" },
        "new_files": { "type": "array", "items": { "type": "string" } },
        "changed": { "type": "array", "items": { "$ref": "#/$defs/entryFile" } }
      }
    },
    "local_entries": {
      "type": "array",
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["key", "path", "approved"],
        "properties": {
          "key": { "type": "string" },
          "path": { "type": "string" },
          "approved": { "type": "boolean" }
        }
      }
    }
  },
  "$defs": {
    "entryFile": {
      "type": "object",
      "additionalProperties": false,
      "required": ["key", "path"],
      "properties": {
        "key": { "type": "string" },
        "path": { "type": "string" }
      }
    }
  }
}
```

## Example

An instance where a config file was changed in game, a mod was tried from the app, and the previous build of the local mod `custom-tweaks` waits for the next load, in a repository where `iris` is not downloaded yet and `custom-tweaks` is not approved.

```json
{
  "socle": { "repository": "0.1.0", "cli": "0.1.0", "gap": "equal" },
  "instance": {
    "path": "/home/me/.local/share/ModrinthApp/profiles/My Pack",
    "variant": "default",
    "drift": ["config/jei-client.toml"],
    "disabled": [],
    "unknown_files": [
      {
        "path": "mods/appleskin-neoforge-mc1.21-3.0.5.jar",
        "sha1": "a9993e364706816aba3e25717850c26c9cd0d89d",
        "sha512": "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
      }
    ],
    "outdated_files": [{ "path": "mods/custom-tweaks-0.3.jar", "local_entry": "custom-tweaks" }]
  },
  "missing_files": [{ "key": "iris", "path": "mods/iris-neoforge-1.8.1.jar" }],
  "manifest": { "stale": false, "new_files": [], "changed": [] },
  "local_entries": [{ "key": "custom-tweaks", "path": "mods-local/custom-tweaks-0.3.jar", "approved": false }]
}
```
