# `modpack status --json`

`modpack status` reports the state of a pack on this machine: the socle version, the pack's Modrinth instance, the variant loaded there, drift, jars of the instance the manifest does not know, how `mods/` and `mods-local/` of the repository compare to the manifest, and the local mods with their approval. It reads files only: it writes nothing and makes no network call.

With `--json`, the report is one JSON object on stdout. This format is the contract read by the socle's return skill (`modpack-return`). Logs go to stderr and never mix with it.

## Exit code

- `0`: the report was produced, whatever it contains. Findings (drift, unknown jars, stale manifest) do not change the exit code.
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
| `instance.drift` | Files the CLI wrote into the instance that changed or were deleted since, relative to the instance, with `/` separators. |
| `instance.disabled` | Jars of `drift` that the Modrinth App disabled: renamed `<jar>.disabled`, content unchanged. `modpack variant load <variant> --force` enables them again. |
| `instance.unknown_jars` | Jars of the instance's `mods/` whose sha1 and sha512 match no manifest entry, whatever their file name, with their hashes so they can be looked up on the platforms. |
| `missing_jars` | Manifest entries whose jar is absent from `mods/` of the repository, or replaced by an older jar `modpack install` downloaded and nobody changed since: `modpack install` downloads them. A local mod listed here lost its jar from `mods-local/`: git restores it. |
| `manifest.stale` | `true` when `new_jars` or `changed` is not empty: `modpack manifest` regenerates the manifest. |
| `manifest.new_jars` | Jars of `mods/` or `mods-local/` of the repository that no entry lists, except the ones `modpack install` downloaded and nobody changed since: those are removed by the next `modpack install`, never added back. |
| `manifest.changed` | Entries whose jar is in `mods/` or `mods-local/` of the repository with other hashes than the manifest's. |
| `local_mods` | The local mods: jars tracked by git in `mods-local/` (see [local mods](local-mods.md)). `approved` is `true` when the approval recorded in the manifest matches the jar; the publish workflow refuses to publish while one is `false`, and `modpack approve <key>` approves it. |

Only hashes are compared: the report never asks a platform about a jar.

A jar listed in `missing_jars` does not make the manifest stale: a missing jar means "not installed on this machine", never "removed from the pack", and `modpack manifest` keeps its entry. It is the usual case in a fresh clone, since git does not hold the jars. A mod leaves the pack only through `modpack remove <key>`, which also takes its key out of the variants.

Lists are sorted. Paths of jars are relative (`mods/<name>`, `mods-local/<name>`), to the instance for `unknown_jars`, to the repository otherwise.

## Schema

JSON Schema (draft 2020-12). It describes this version exactly. A later version of the CLI may add fields, so a reader ignores the fields it does not know.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "modpack status",
  "type": "object",
  "additionalProperties": false,
  "required": ["socle", "instance", "missing_jars", "manifest", "local_mods"],
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
          "required": ["path", "variant", "drift", "disabled", "unknown_jars"],
          "properties": {
            "path": { "type": "string" },
            "variant": { "type": ["string", "null"] },
            "drift": { "type": "array", "items": { "type": "string" } },
            "disabled": { "type": "array", "items": { "type": "string" } },
            "unknown_jars": {
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
            }
          }
        }
      ]
    },
    "missing_jars": { "type": "array", "items": { "$ref": "#/$defs/jar" } },
    "manifest": {
      "type": "object",
      "additionalProperties": false,
      "required": ["stale", "new_jars", "changed"],
      "properties": {
        "stale": { "type": "boolean" },
        "new_jars": { "type": "array", "items": { "type": "string" } },
        "changed": { "type": "array", "items": { "$ref": "#/$defs/jar" } }
      }
    },
    "local_mods": {
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
    "jar": {
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

An instance where a config file was changed in game and a mod was tried from the app, in a repository where `iris` is not downloaded yet and the local mod `custom-tweaks` is not approved.

```json
{
  "socle": { "repository": "0.1.0", "cli": "0.1.0", "gap": "equal" },
  "instance": {
    "path": "/home/me/.local/share/ModrinthApp/profiles/My Pack",
    "variant": "default",
    "drift": ["config/jei-client.toml"],
    "disabled": [],
    "unknown_jars": [
      {
        "path": "mods/appleskin-neoforge-mc1.21-3.0.5.jar",
        "sha1": "a9993e364706816aba3e25717850c26c9cd0d89d",
        "sha512": "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
      }
    ]
  },
  "missing_jars": [{ "key": "iris", "path": "mods/iris-neoforge-1.8.1.jar" }],
  "manifest": { "stale": false, "new_jars": [], "changed": [] },
  "local_mods": [{ "key": "custom-tweaks", "path": "mods-local/custom-tweaks-0.3.jar", "approved": false }]
}
```
