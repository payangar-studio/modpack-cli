---
name: modpack-return
description: "Bring what changed in this pack's Modrinth instance back into the repository: jars tried from the app that the manifest does not know, the config files they created, and files the CLI wrote that were changed in game (drift). Use when the user wants to keep a mod they tried in the instance, asks what changed in the instance, or when `modpack mode load` or `modpack status` reports drift or unknown jars. Proposes every change and makes none without the user's go."
allowed-tools:
  - Bash
  - Read
  - Grep
  - Glob
  - AskUserQuestion
---

# Return from the instance

The repository feeds the instance one way only. Nothing comes back by itself: when the user tries a mod from the Modrinth App or changes a setting in game, this skill finds it, explains it and, once the user agrees, brings it into the repository with the CLI.

## Rules

- **Nothing without the user's go.** Steps 1 to 5 only read. Show the whole proposal first, then apply only what the user accepted.
- **The CLI writes the manifest and the modes.** Never edit `mods.lock.toml` by hand, never copy a jar into `mods/` yourself: `modpack add` does both, and its `--mode` writes `modes.toml`.
- **Never write into the instance.** It is the user's; loading a mode is what rewrites it.
- **Every command stands alone.** Each command runs in a fresh shell: no shell variable, no `cd` carries over to the next one. Write paths out in full every time, and take values from the previous outputs, not from variables.
- **Secrets never pass through the conversation.** When the CurseForge key is missing, say so and point to `modpack auth`, which the user runs in their own terminal.

Run every command from the repository root.

## 1. Read the status

```bash
modpack status --json
```

One JSON object on stdout (exit code 0 whatever it reports). What matters here:

- `instance`: `null` means this machine has no instance for the pack. Say so, suggest `modpack setup`, and stop.
- `instance.path`: the instance directory. `instance.mode`: the mode loaded last (`null`: none yet).
- `instance.unknown_jars`: jars of the instance's `mods/` that no manifest entry has, by hash, with `path` relative to the instance, `sha1` and `sha512`.
- `instance.drift`: files the CLI wrote into the instance that were changed or deleted since, relative to the instance.
- `instance.disabled`: the jars of `drift` the user disabled in the Modrinth App (renamed `<jar>.disabled`).
- `manifest.stale`, `missing_jars`: state of the repository itself. Mention them, they are not this skill's job (`modpack manifest`, `modpack install`).

When `unknown_jars` and `drift` are both empty, say that the instance holds nothing to bring back, and stop.

## 2. Identify each unknown jar

The CLI identifies a jar on every platform when it adds it. Run that identification in a throwaway copy of the repository's lists, so that the repository itself is not touched. The copy lives at `/tmp/modpack-return-<name of the repository directory>`, called `<probe>` below: write that path out in full in each command. Quote the paths: instance directories often hold spaces.

Create the copy, in one command:

```bash
rm -rf "<probe>" && mkdir -p "<probe>/.modpack" && cp .modpack/socle.toml "<probe>/.modpack/" && cp mods.lock.toml modes.toml "<probe>/"
```

Then, once per unknown jar:

```bash
(cd "<probe>" && modpack add "<instance.path>/<path>")
```

For each jar, `modpack add` prints the key it chose and the platforms that know it (`added `<key>` (mods/<file>), on modrinth, curseforge` or `found on no platform`), then the required dependencies missing from the pack. A line `curseforge skipped: CurseForge needs a Core API key` means CurseForge was not asked: its availability there is unknown, not absent.

Then read each new entry in `<probe>/mods.lock.toml` (the `[[mods]]` block whose `key` was printed):

- `[mods.modrinth]`: `project_id`, `version_id`, `dependencies` (required ones, as Modrinth project ids). Name and page: `curl -s https://api.modrinth.com/v2/project/<project_id>` (fields `title`, `slug`, `license.id`). The version: `curl -s https://api.modrinth.com/v2/version/<version_id>` (fields `version_number`, `game_versions`, `loaders`, and `dependencies` with their `dependency_type`, which also lists `incompatible` projects).
- `[mods.curseforge]`: `projectID`, `fileID`, `dependencies` when present.
- `[mods.env]`: the sides the mod runs on, `client` and `server`, each `required`, `optional` or `unsupported`. On a jar found on no platform it is only the default (`required` on both): read `environment` in the jar's `fabric.mod.json` (`client`, `server` or `*`), otherwise the mod's page, and say the side is unconfirmed when neither tells.
- Neither platform table: the jar is on no platform. Read its own metadata to name it: `unzip -p "<instance.path>/<path>" META-INF/neoforge.mods.toml` (or `META-INF/mods.toml`, `fabric.mod.json`).

Note the jar's mod id too, for step 3: `modId` in `META-INF/neoforge.mods.toml` (or `META-INF/mods.toml`), `id` in `fabric.mod.json`.

Also compare with the pack:

- **A newer or older version of a mod the pack has**: the same `project_id` (or `projectID`) as an entry of the real `mods.lock.toml`. Usually the user updated it from the app, and the old jar then shows up in `drift` as deleted.
- **Another target**: `game_versions` or `loaders` that do not include the `[pack]` table's `minecraft` and `loader`.
- **Dependencies**: for each one, whether the pack has it (`grep -n '<id>' mods.lock.toml`) or not, and its name from the platform. An `incompatible` project the pack has is a conflict to show.
- **Sides**: read the `side` conditions of `modes.toml`: in the root `[conditions]` (every mode), `[default.conditions]`, and `[modes.<name>.conditions]`. A mode with `side = "client"` cannot hold a mod whose `env.client` is `unsupported`, nor a mode with `side = "server"` one whose `env.server` is `unsupported`. A mode without `side` can hold any mod. Note, for each jar, the modes that can hold it and the ones that cannot.

Remove the copy when done:

```bash
rm -rf "<probe>"
```

## 3. Find the config files the tried mods created

A mod creates its config files the first time the game runs with it. These files are not drift (the CLI never wrote them), so `modpack status` does not report them. For each unknown jar, list the candidates, one command each:

- Created after the jar landed in the instance: `find "<instance.path>/config" -type f -newerBm "<instance.path>/<path>"`. When `find` says the file system has no birth time, use `-newer` instead, which also lists files other mods rewrote at launch.
- Named after the mod: `find "<instance.path>/config" -ipath '*<mod id>*'`.

Then drop from these lists:

- the files the CLI wrote, listed under `[files]` in `<instance.path>/.modpack-state.toml` (read it with Read): their changes are drift, step 4;
- the files the pack already has, at `config/<rest>` or `overrides/<instance.mode>/config/<rest>` in the repository.

A file in both lists most likely belongs to the tried mod. A file only created after the jar may belong to another mod the game ran for the first time, or to the game itself: say so rather than attribute it. No candidate usually means the game has not run with the mod yet.

## 4. Sort out the drift

For each path of `drift`, relative to the instance:

- **A config file** (`config/<rest>`): it comes from `overrides/<mode>/config/<rest>` when that file exists for the loaded mode, from `config/<rest>` otherwise. Show the difference: `diff -u <repository file> "<instance.path>/config/<rest>"`. When the file is gone from the instance, say so.
- **A jar** (`mods/<file>`): a jar of the mode was deleted or replaced in the instance. Find its entry by its `path` in `mods.lock.toml`. Usually the user removed the mod from the app, or updated it (step 2 then shows the new jar). When it is in `disabled`, the user turned the mod off in the app.

## 5. Propose

Present everything at once, mod by mod, then file by file. For each unknown jar:

- what it is (name, version, link to its page), the sides it runs on;
- where it is available: Modrinth, CurseForge (or unknown without the key), neither. A mod on neither platform cannot be downloaded on another machine without a direct URL, and blocks both published packs unless its license allows embedding it;
- its required dependencies, and which ones the pack lacks;
- conflicts or target mismatches found in step 2;
- the modes it can go into, after the sides, and the proposal: integrate it, or leave it in the instance. Propose only modes that can hold it:
  - every mode can: integrate it into every mode, or into the modes the user picks;
  - some modes cannot: integrate it into the modes that can, which makes the others remove it (`default` among them keeps it in the default selection and puts it in the `remove` list of every mode not named);
  - no mode can: do not propose integrating it, and say why.
- the config files step 3 attributes to it: bring each into `config/<rest>`, or leave it. A file the user never changed only holds the mod's defaults: bringing it pins them, leaving it lets the mod write its own.

For each drifted config file: keep the change in the place it came from, or move it to the other one (`config/` applies to every mode, `overrides/<mode>/config/` to that mode only), or drop it. For a jar deleted or replaced in the instance: remove the mod from the pack, update it, or restore it. For a jar disabled in the app: remove the mod from the pack or from the loaded mode, or enable it again.

Ask the user which proposals to apply, with AskUserQuestion when there are few choices, in plain text otherwise.

## 6. Apply what the user accepted

- **Integrate an unknown jar**:
  ```bash
  modpack add "<instance.path>/<path>"                                 # every mode
  modpack add "<instance.path>/<path>" --mode default --mode <mode>    # these modes only
  ```
  `--mode` repeats, once per mode the mod goes into; `default` names the default mode. Without `default`, the default mode removes the mod and each named mode adds it. With `default`, every mode not named removes it: a client-only mod in a pack with `default` (`side = "client"`) and `server` (`side = "server"`) goes in with `--mode default`. Add `--with-deps` when the user wants the missing required dependencies too, which go into the same modes; without it, the command only lists them. A jar on no platform needs its direct download URL: `--url <url>`, from the user or the mod's official release page, never guessed.
- **Update a mod the pack has to the version tried in the instance**: when it is on Modrinth, `modpack add <project_id> --version <version_id>` replaces it under its key and keeps it in its modes. A local jar added with `modpack add <jar>` would get a second key instead.
- **Remove a mod the user deleted from the instance**: `modpack remove <key>`, which also takes it out of every mode.
- **Keep a config file** (a drifted one, or one a tried mod created): copy the instance's file over the chosen repository file, creating its directory, in one command:
  ```bash
  mkdir -p "$(dirname <repository file>)" && cp "<instance.path>/config/<rest>" <repository file>
  ```
  When a change moves from `config/` to `overrides/<mode>/config/`, the base file stays as it was.
- **Drop a config change**: nothing to do in the repository; the next `modpack mode load <mode> --force` rewrites the instance's file.

Never edit `modes.toml` to place a new mod: `--mode` does it. Editing it by hand stays for a mod already in the pack, whose modes change.

## 7. Check and hand over

1. `modpack status --json`: the integrated jars are no longer in `unknown_jars`, and `manifest.stale` is `false`.
2. `modpack test --json`, for the static conditions of the modes, `side` included (a failure is reported, not fixed silently).
3. `git status --short` and `git diff --stat`: show what changed in the repository and suggest a commit. Do not commit without the user's go.
4. The drift stays until the mode is loaded again. Once the changes they wanted are in the repository, the user (or you, with their go) runs `modpack mode load <mode> --force`, which rewrites the instance from the repository and resets the drift reference.
