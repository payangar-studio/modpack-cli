# `variants.toml`

`variants.toml`, at the root of a pack repository, lists the variants of the pack and the conditions each variant must meet. A variant is a selection of the pack's mods plus its own configuration: the full pack, a dedicated server, a lighter client. Every command that reads the file refuses it, with exit code 2, when it breaks a rule below.

## Example

```toml
# Global conditions: every variant, the default one included.
[conditions]
deps-complete = true

[default]
description = "Full pack"
remove = ["hardcore-tweaks"]

[default.conditions]
side = "client"

[variants.server]
description = "Dedicated server"
remove = ["sodium", "iris", "jei"]

[variants.server.conditions]
side = "server"
server-starts = { timeout = "180s" }

[variants.hardcore]
description = "Hardcore"
add = ["hardcore-tweaks"]
```

The default variant holds every mod but `hardcore-tweaks`. `server` holds the default selection without `sodium`, `iris` and `jei`. `hardcore` holds the default selection plus `hardcore-tweaks`: every mod of the pack.

## Variants

There is one level of variants. The default variant is the full pack; every other variant is defined from it, never from another variant. Mods are named by their key in `mods.lock.toml`.

| Table | Fields |
|---|---|
| `[default]` | `description`, `remove`, `conditions` |
| `[variants.<name>]` | `description`, `add`, `remove`, `conditions` |

- `description`: shown by `modpack variant list`.
- `remove`: keys taken out. The default variant starts from every mod of the manifest; another variant starts from the default variant's selection.
- `add` (other variants only): keys added to the default variant's selection. A key added must be one the default variant removes: a mod of one variant only is in the manifest, removed from the default variant and added by its variant.
- `conditions`: the variant's own conditions, see below.

`modpack add <mod> --variant <variant>` puts a new mod in some variants only, and writes these lists for it. `--variant` repeats, and `default` names the default variant:

- Without `--variant`, the mod is in every variant.
- `--variant server --variant hardcore`: the default variant removes the mod, `server` and `hardcore` add it.
- `--variant default --variant hardcore`: the mod stays in the default selection, and every other variant not named (here `server`) removes it. This is how a client-only mod leaves a `side = "server"` variant.

A file without `[default]` is a default variant with every mod. The name `default` is reserved: `[variants.default]` is refused. So are a nested variant (`[variants.server.lowend]`), an unknown field, and a key the manifest does not have. `modpack remove <key>` takes the key out of every variant.

## Configuration

A variant's configuration is the base `config/`, then `overrides/<variant>/config/`, whose files replace the base files at the same path. The default variant's overrides are in `overrides/default/config/`. `.gitkeep` files are never copied. `modpack variant load` writes this configuration into the instance, and `modpack pack` puts it under `overrides/` in the packs.

## Conditions

Conditions are set in a `conditions` table: at the root of the file for every variant, the default one included, or in a variant for that variant only. A condition set at the root cannot be set again in a variant. An unknown condition is refused.

| Condition | Values | What `modpack test` checks |
|---|---|---|
| `side` | `"client"` or `"server"` | Every mod of the variant supports that side, after its `env` in `mods.lock.toml`: a mod whose `env.client` (or `env.server`) is `unsupported` fails it. |
| `deps-complete` | `true` or `false` | Every required dependency each mod of the variant declares (the `dependencies` of its platform tables in `mods.lock.toml`) is a mod of the variant. A dependency that is in the manifest but not in the variant fails it, and so does one no mod of the manifest provides. |
| `server-starts` | `true`, or `{ timeout = "<duration>" }` | A dedicated server with the variant's mods reaches the `Done` line of its log within the timeout, 300 seconds by default. A crash or the timeout fails it. |
| `client-starts` | `true` or `false` | Not implemented yet: always skipped. |

`false` turns a condition off. A duration is a whole number followed by `s`, `m` or `h`: `90s`, `3m`, `1h`.

`server-starts` installs the server of the pack's loader and Minecraft version once, with the loader's official installer, into the machine's cache (`<user cache dir>/modpack/servers`). Each check then starts a copy of it with the variant's jars, except those whose `env.server` is `unsupported`, the base configuration, then the variant's overrides. It handles the `fabric` and `neoforge` loaders and is skipped for the others. It needs Java, from `JAVA_HOME` or the `PATH`, and is skipped without it.

## Checking

`modpack test [variant] [--condition <name>] [--json]` checks the conditions of a variant, or of every variant: the global conditions, then the variant's own. Each condition passes, fails or is skipped, with the reason. The command exits with 3 when a condition fails; a skipped condition does not count.

The rules of this file are checked by `modpack manifest`, which the `pre-commit` hook runs, so that a commit cannot introduce a broken `variants.toml`. `modpack variant load`, `modpack test` and `modpack pack` check them too.
