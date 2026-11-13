# `modes.toml`

`modes.toml`, at the root of a pack repository, lists the modes of the pack and the conditions each mode must meet. A mode is a selection of the pack's mods plus its own configuration: the full pack, a dedicated server, a lighter client. Every command that reads the file refuses it, with exit code 2, when it breaks a rule below.

## Example

```toml
# Global conditions: every mode, the default one included.
[conditions]
deps-complete = true

[default]
description = "Full pack"
remove = ["hardcore-tweaks"]

[default.conditions]
side = "client"

[modes.server]
description = "Dedicated server"
remove = ["sodium", "iris", "jei"]

[modes.server.conditions]
side = "server"
server-starts = { timeout = "180s" }

[modes.hardcore]
description = "Hardcore"
add = ["hardcore-tweaks"]
```

The default mode holds every mod but `hardcore-tweaks`. `server` holds the default selection without `sodium`, `iris` and `jei`. `hardcore` holds the default selection plus `hardcore-tweaks`: every mod of the pack.

## Modes

There is one level of modes. The default mode is the full pack; every other mode is defined from it, never from another mode. Mods are named by their key in `mods.lock.toml`.

| Table | Fields |
|---|---|
| `[default]` | `description`, `remove`, `conditions` |
| `[modes.<name>]` | `description`, `add`, `remove`, `conditions` |

- `description`: shown by `modpack mode list`.
- `remove`: keys taken out. The default mode starts from every mod of the manifest; another mode starts from the default mode's selection.
- `add` (other modes only): keys added to the default mode's selection. A key added must be one the default mode removes: a mod of one mode only is in the manifest, removed from the default mode and added by its mode.
- `conditions`: the mode's own conditions, see below.

`modpack add <mod> --mode <mode>` puts a new mod in some modes only, and writes these lists for it. `--mode` repeats, and `default` names the default mode:

- Without `--mode`, the mod is in every mode.
- `--mode server --mode hardcore`: the default mode removes the mod, `server` and `hardcore` add it.
- `--mode default --mode hardcore`: the mod stays in the default selection, and every other mode not named (here `server`) removes it. This is how a client-only mod leaves a `side = "server"` mode.

A file without `[default]` is a default mode with every mod. The name `default` is reserved: `[modes.default]` is refused. So are a nested mode (`[modes.server.lowend]`), an unknown field, and a key the manifest does not have. `modpack remove <key>` takes the key out of every mode.

## Configuration

A mode's configuration is the base `config/`, then `overrides/<mode>/config/`, whose files replace the base files at the same path. The default mode's overrides are in `overrides/default/config/`. `.gitkeep` files are never copied. `modpack mode load` writes this configuration into the instance, and `modpack pack` puts it under `overrides/` in the packs.

## Conditions

Conditions are set in a `conditions` table: at the root of the file for every mode, the default one included, or in a mode for that mode only. A condition set at the root cannot be set again in a mode. An unknown condition is refused.

| Condition | Values | What `modpack test` checks |
|---|---|---|
| `side` | `"client"` or `"server"` | Every mod of the mode supports that side, after its `env` in `mods.lock.toml`: a mod whose `env.client` (or `env.server`) is `unsupported` fails it. |
| `deps-complete` | `true` or `false` | Every required dependency each mod of the mode declares (the `dependencies` of its platform tables in `mods.lock.toml`) is a mod of the mode. A dependency that is in the manifest but not in the mode fails it, and so does one no mod of the manifest provides. |
| `server-starts` | `true`, or `{ timeout = "<duration>" }` | A dedicated server with the mode's mods reaches the `Done` line of its log within the timeout, 300 seconds by default. A crash or the timeout fails it. |
| `client-starts` | `true` or `false` | Not implemented yet: always skipped. |

`false` turns a condition off. A duration is a whole number followed by `s`, `m` or `h`: `90s`, `3m`, `1h`.

`server-starts` installs the server of the pack's loader and Minecraft version once, with the loader's official installer, into the machine's cache (`<user cache dir>/modpack/servers`). Each check then starts a copy of it with the mode's jars, except those whose `env.server` is `unsupported`, the base configuration, then the mode's overrides. It handles the `fabric` and `neoforge` loaders and is skipped for the others. It needs Java, from `JAVA_HOME` or the `PATH`, and is skipped without it.

## Checking

`modpack test [mode] [--condition <name>] [--json]` checks the conditions of a mode, or of every mode: the global conditions, then the mode's own. Each condition passes, fails or is skipped, with the reason. The command exits with 3 when a condition fails; a skipped condition does not count.

The rules of this file are checked by `modpack manifest`, which the `pre-commit` hook runs, so that a commit cannot introduce a broken `modes.toml`. `modpack mode load`, `modpack test` and `modpack pack` check them too.
