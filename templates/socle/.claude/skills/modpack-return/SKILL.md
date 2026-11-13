---
name: modpack-return
description: "Bring what changed in this pack's Modrinth instance back into the repository: jars tried from the app that the manifest does not know, and files the CLI wrote that were changed in game (drift). Use when the user wants to keep a mod they tried in the instance, asks what changed in the instance, or when `modpack mode load` or `modpack status` reports drift or unknown jars. Proposes every change and makes none without the user's go."
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

- **Nothing without the user's go.** Steps 1 to 4 only read. Show the whole proposal first, then apply only what the user accepted.
- **The CLI writes the manifest.** Never edit `mods.lock.toml` by hand, never copy a jar into `mods/` yourself: `modpack add` does both.
- **Never write into the instance.** It is the user's; loading a mode is what rewrites it.
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

The CLI identifies a jar on every platform when it adds it. Run that identification in a throwaway copy of the repository's lists, so that the repository itself is not touched:

```bash
probe=$(mktemp -d)
mkdir "$probe/.modpack"
cp .modpack/socle.toml "$probe/.modpack/"
cp mods.lock.toml modes.toml "$probe/"
# Once per unknown jar:
(cd "$probe" && modpack add "<instance.path>/<path>")
```

Quote the paths: instance directories often hold spaces. For each jar, `modpack add` prints the key it chose and the platforms that know it (`added `<key>` (mods/<file>), on modrinth, curseforge` or `found on no platform`), then the required dependencies missing from the pack. A line `curseforge skipped: CurseForge needs a Core API key` means CurseForge was not asked: its availability there is unknown, not absent.

Then read each new entry in `$probe/mods.lock.toml` (the `[[mods]]` block whose `key` was printed):

- `[mods.modrinth]`: `project_id`, `version_id`, `dependencies` (required ones, as Modrinth project ids). Name and page: `curl -s https://api.modrinth.com/v2/project/<project_id>` (fields `title`, `slug`, `license.id`). The version: `curl -s https://api.modrinth.com/v2/version/<version_id>` (fields `version_number`, `game_versions`, `loaders`, and `dependencies` with their `dependency_type`, which also lists `incompatible` projects).
- `[mods.curseforge]`: `projectID`, `fileID`, `dependencies` when present.
- `env`: the sides the mod runs on (`client`, `server`: `required`, `optional` or `unsupported`).
- Neither platform table: the jar is on no platform. Read its own metadata to name it: `unzip -p <jar> META-INF/neoforge.mods.toml` (or `META-INF/mods.toml`, `fabric.mod.json`).

Also compare with the pack:

- **A newer or older version of a mod the pack has**: the same `project_id` (or `projectID`) as an entry of the real `mods.lock.toml`. Usually the user updated it from the app, and the old jar then shows up in `drift` as deleted.
- **Another target**: `game_versions` or `loaders` that do not include the `[pack]` table's `minecraft` and `loader`.
- **Dependencies**: for each one, whether the pack has it (`grep -n '<id>' mods.lock.toml`) or not, and its name from the platform. An `incompatible` project the pack has is a conflict to show.

Remove the probe when done: `rm -rf "$probe"`.

## 3. Sort out the drift

For each path of `drift`, relative to the instance:

- **A config file** (`config/<rest>`): it comes from `overrides/<mode>/config/<rest>` when that file exists for the loaded mode, from `config/<rest>` otherwise. Show the difference: `diff -u <repository file> "<instance.path>/config/<rest>"`. When the file is gone from the instance, say so.
- **A jar** (`mods/<file>`): a jar of the mode was deleted or replaced in the instance. Find its entry by its `path` in `mods.lock.toml`. Usually the user removed the mod from the app, or updated it (step 2 then shows the new jar). When it is in `disabled`, the user turned the mod off in the app.

## 4. Propose

Present everything at once, mod by mod, then file by file. For each unknown jar:

- what it is (name, version, link to its page), the sides it runs on;
- where it is available: Modrinth, CurseForge (or unknown without the key), neither. A mod on neither platform cannot be downloaded on another machine without a direct URL, and blocks both published packs unless its license allows embedding it;
- its required dependencies, and which ones the pack lacks;
- conflicts or target mismatches found in step 2;
- the proposal: integrate it, into every mode or into one mode only, or leave it in the instance.

For each drifted config file: keep the change in the place it came from, or move it to the other one (`config/` applies to every mode, `overrides/<mode>/config/` to that mode only), or drop it. For a jar deleted or replaced in the instance: remove the mod from the pack, update it, or restore it. For a jar disabled in the app: remove the mod from the pack or from the loaded mode, or enable it again.

Ask the user which proposals to apply, with AskUserQuestion when there are few choices, in plain text otherwise.

## 5. Apply what the user accepted

- **Integrate an unknown jar**:
  ```bash
  modpack add "<instance.path>/<path>"                   # every mode
  modpack add "<instance.path>/<path>" --mode <mode>     # one mode only
  ```
  Add `--with-deps` when the user wants the missing required dependencies too; without it, the command only lists them. A jar on no platform needs its direct download URL: `--url <url>`, from the user or the mod's official release page, never guessed.
- **Update a mod the pack has to the version tried in the instance**: when it is on Modrinth, `modpack add <project_id> --version <version_id>` replaces it under its key and keeps it in its modes. A local jar added with `modpack add <jar>` would get a second key instead.
- **Remove a mod the user deleted from the instance**: `modpack remove <key>`, which also takes it out of every mode.
- **Keep a config change**: copy the instance's file over the chosen repository file, creating its directory if needed:
  ```bash
  mkdir -p "$(dirname <repository file>)"
  cp "<instance.path>/config/<rest>" <repository file>
  ```
  When the change moves from `config/` to `overrides/<mode>/config/`, the base file stays as it was.
- **Drop a config change**: nothing to do in the repository; the next `modpack mode load <mode> --force` rewrites the instance's file.

Never edit `modes.toml` to add a mod to a mode: `--mode` does it. To leave a mod out of some modes only, edit their `remove` lists in `modes.toml` by hand, which is the pack's content.

## 6. Check and hand over

1. `modpack status --json`: the integrated jars are no longer in `unknown_jars`, and `manifest.stale` is `false`.
2. `modpack test --json`, for the static conditions of the modes (a failure is reported, not fixed silently).
3. `git status --short` and `git diff --stat`: show what changed in the repository and suggest a commit. Do not commit without the user's go.
4. The drift stays until the mode is loaded again. Once the changes they wanted are in the repository, the user (or you, with their go) runs `modpack mode load <mode> --force`, which rewrites the instance from the repository and resets the drift reference.
