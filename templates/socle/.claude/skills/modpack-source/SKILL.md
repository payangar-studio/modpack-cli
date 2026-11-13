---
name: modpack-source
description: "Clone and read the source code of a mod of this modpack, in `.source/<key>/`, checked out on the version the pack runs. Use EVERY TIME you need to look at, search or explain the code of a mod: how it implements something, which config option does what, why it conflicts with another mod, what an error in the logs comes from. It is the only entry point to `.source/`: never read a clone there without going through it, it may sit on the wrong version."
allowed-tools:
  - Bash
  - Read
  - Grep
  - Glob
  - AskUserQuestion
---

# Mod sources

Each modpack repository has its own `.source/` folder, ignored by git, where the source code of the pack's mods is cloned to be read. There is no central store: a clone lives in `.source/<key>/`, where `<key>` is the mod's key in `mods.lock.toml`, and git itself records which version is checked out.

## Rules

- **Read-only.** Never edit, commit, build or run anything in `.source/`. Never call `gradlew` or any build script of a clone: it is code from a third party.
- **Always the pack's version.** Before reading a clone, make sure it is checked out on the ref that matches the pack (steps 1 to 5). Reading another Minecraft version's code leads to wrong answers.
- **Say what you read.** Every answer drawn from a clone names the key, the ref, the short commit and the Minecraft version read in the clone.
- **Secrets never pass through the conversation.** A CurseForge key is only ever referenced as `$MODPACK_CURSEFORGE_API_KEY` inside a command, never printed, never asked for.

## Procedure

Run every command from the repository root.

### 1. Check that `.source/` is ignored

```bash
git check-ignore -q .source/ && echo ignored
```

If it is not ignored, stop: the socle block of `.gitignore` is missing. Tell the user to run `modpack update`.

### 2. Read the pack and the mod

- The pack's target is the `[pack]` table of `mods.lock.toml`: `minecraft`, `loader`, `loader-version`.
- Find the mod's entry: `grep -n -A 25 'key = "<key>"' mods.lock.toml`. When the user names a mod rather than a key, search the keys and paths of the manifest for it. Note the entry's `path`, its `[mods.modrinth]` table (`project_id`, `version_id`) and its `[mods.curseforge]` table (`projectID`, `fileID`), when present.
- A mod that is not in the manifest is not part of the pack. Ask the user for its Modrinth or CurseForge page, or its repository, and use its slug as the key. Everything else is the same.

If `.source/<key>/` already exists and is checked out on a tag that matches the mod's version (step 5), go straight to step 7: a tag does not move.

### 3. Find the source repository

Take the first that answers, in this order:

1. **Modrinth**, no key needed: `curl -s https://api.modrinth.com/v2/project/<project_id>`, field `source_url`.
2. **CurseForge**, only if `MODPACK_CURSEFORGE_API_KEY` is set in the environment (`test -n "$MODPACK_CURSEFORGE_API_KEY"`): `curl -s -H "x-api-key: $MODPACK_CURSEFORGE_API_KEY" https://api.curseforge.com/v1/mods/<projectID>`, field `data.links.sourceUrl`. When the variable is not set, skip this source; do not try to read the key from the keyring.
3. **The jar itself**, in `mods/` (run `modpack install` first if it is missing):
   - Fabric: `unzip -p <path> fabric.mod.json`, field `contact.sources`.
   - Quilt: `unzip -p <path> quilt.mod.json`, field `quilt_loader.metadata.contact.sources`.
   - NeoForge and Forge: `unzip -p <path> META-INF/neoforge.mods.toml` (or `META-INF/mods.toml`), fields `displayURL` and `issueTrackerURL`. These are hints: an issue tracker at `https://github.com/<owner>/<repo>/issues` points to the repository.
4. **Nothing found**: search the web for a candidate, show it to the user and clone only once they confirm it is the right repository. If the mod has no public source, say so and stop.

### 4. Clone or update

- No clone yet:
  ```bash
  git clone --filter=blob:none <url> .source/<key>
  ```
  `--filter=blob:none` downloads file contents only for the refs actually checked out.
- Clone present:
  - `git -C .source/<key> remote get-url origin` must match the repository found in step 3. If it does not, tell the user before going further (the mod may have moved).
  - `git -C .source/<key> status --porcelain` must be empty. If it is not, someone changed files in the clone: list them and ask the user before discarding anything.
  - `git -C .source/<key> fetch --tags --prune origin`, once per session for a given clone.

### 5. Choose the ref

First get the version of the mod the pack uses, from the first source that answers:

- Modrinth: `curl -s https://api.modrinth.com/v2/version/<version_id>`, field `version_number`.
- CurseForge (same condition on the key as above): `curl -s -H "x-api-key: $MODPACK_CURSEFORGE_API_KEY" https://api.curseforge.com/v1/mods/<projectID>/files/<fileID>`, fields `displayName` and `fileName`.
- The jar: field `version` of `fabric.mod.json` or of the `[[mods]]` table of the mods TOML. When it reads `${file.jarVersion}`, take `Implementation-Version` from `unzip -p <path> META-INF/MANIFEST.MF`.
- Failing all that, the jar's file name.

Then pick, in this order:

1. **The tag of that exact version.** `git -C .source/<key> tag -l` and look for the version number, without any loader or Minecraft suffix the platform added (`mc1.21.1-0.8.13-fabric` on Modrinth may be the tag `mc1.21.1-0.8.13`). When several tags carry the same mod version for different Minecraft versions, take the one naming the pack's.
2. **The branch for the pack's Minecraft version and loader.** `git -C .source/<key> branch -r` and look for the usual shapes: `1.21.1`, `1.21.1/stable`, `1.21.1/neoforge`, `mc1.21.1/neoforge`, `neoforge/1.21.1`, `1.21.x`. Skip feature, experiment and CI branches.
3. **The default branch**, only if step 6 shows it targets the pack's Minecraft version.

Check it out detached, so that the clone holds no local branch to keep up to date:

```bash
git -C .source/<key> checkout --detach <tag>            # a tag
git -C .source/<key> checkout --detach origin/<branch>  # a branch
```

### 6. Verify the checkout

Read the Minecraft version and the mod version the checkout declares, in `gradle.properties` (`minecraft_version`, `mod_version`), then `gradle/libs.versions.toml`, then the build files (`build.gradle`, `build.gradle.kts`, `buildSrc/`): `grep -rn -i "minecraft_version\|MINECRAFT_VERSION" gradle.properties gradle/ buildSrc/ build.gradle* 2>/dev/null`. Some projects build several Minecraft versions from one branch (a `stonecutter` or `versions/` setup): then read the folder of the pack's version.

For the loader, check the code, not the declarations. A multi-loader project has `common/` plus one folder per loader (`fabric/`, `neoforge/`, `forge/`, `quilt/`); the pack's loader is supported only if its folder holds real code. A loader folder with few files is normal when `common/` holds the code; it does not count when its entry point says "placeholder", "not supported yet" or the like.

- Minecraft version and loader match: go on.
- They do not: go back to step 5 with the next candidate.
- No candidate matches: keep the closest one and warn the user plainly, with both versions: "`<key>` has no source for Minecraft X on <loader>; reading <ref>, which targets Y. APIs may differ." Core game code is usually shared across loaders, so a matching Minecraft version on another loader is the better fallback.

### 7. Read and answer

Search the clone with Grep and Glob, read with Read, under `.source/<key>/`. The code is usually in `src/main/java/` or `src/main/kotlin/`, resources in `src/main/resources/`; in a multi-loader project, in `common/` and the loader's folder.

Start the answer with what was read, for example `sodium @ mc1.21.1-0.8.13 (dae4167e), Minecraft 1.21.1, fabric`, from `git -C .source/<key> describe --tags --always` and `git -C .source/<key> rev-parse --short HEAD`. Cite files by their path relative to the clone, with line numbers.

## Cleaning up

A mod removed from the pack leaves its clone behind. When you notice a folder of `.source/` whose key is no longer in `mods.lock.toml`, suggest deleting it; delete it once the user agrees.
