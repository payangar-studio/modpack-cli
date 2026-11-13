---
name: modpack-audit
description: "Audit what changed in this modpack since the last audit: new and changed mods, known conflicts, where each jar comes from, availability and eligibility on each platform, and configuration left behind by removed mods. Use when asked to audit or review the pack or its mods, before tagging a release, or after a batch of `modpack add` and `modpack remove`. It warns and proposes, it never refuses anything; it writes its report to `audit/last.toml`."
allowed-tools:
  - Bash
  - Read
  - Grep
  - Glob
  - Write
  - AskUserQuestion
---

# Pack audit

The audit is a gap, not a lock. It looks at what changed since the commit the last report recorded, warns about what may hurt the pack, and proposes. It blocks nothing and removes nothing without the user's go.

## Rules

- **Read-only, except two things**: the report `audit/last.toml`, and the config files the user agrees to delete.
- **Say where each warning comes from**: a platform's API, the jar's own metadata, a command's output, or your own knowledge of the mods. A warning from your knowledge is a hypothesis; say so.
- **Never edit `mods.lock.toml` or `variants.toml` to fix a finding.** Propose the command (`modpack remove`, `modpack add`) or the change to `variants.toml`, and let the user decide.
- **Secrets never pass through the conversation.** CurseForge is only ever asked with `$MODPACK_CURSEFORGE_API_KEY` inside a command, when it is set.

Run every command from the repository root.

## 1. Find the base

The report records the audited commit:

```toml
# audit/last.toml
commit = "<full sha of the audited commit>"
date = "<YYYY-MM-DD>"
```

- Report present and `git merge-base --is-ancestor <commit> HEAD` succeeds: the base is that commit.
- Report present but its commit is not an ancestor of `HEAD` (history rewritten, squash merge): take the commit that last wrote the report, `git log -1 --format=%H -- audit/last.toml`, and say so.
- No report: this is the first audit. Every mod is new and every config file is checked; the base is the empty tree, `git hash-object -t tree /dev/null`.

Then check the working tree: `git status --porcelain -- mods.lock.toml variants.toml config overrides`. The report records a commit, so uncommitted changes cannot be part of it. When there are some, ask the user: commit first, or audit the working tree as a preview and write no report.

Get the state of the repository: `modpack status --json`. When `manifest.stale` is `true`, warn that the manifest does not match `mods/` and suggest `modpack manifest`. When `missing_jars` is not empty, run `modpack install` before reading jars (it only downloads into `mods/`, ignored by git).

## 2. What changed

```bash
git diff --stat <base> -- mods.lock.toml variants.toml config overrides
keys() { grep '^key = ' | sed 's/^key = "\(.*\)"$/\1/' | sort; }
comm -13 <(git show <base>:mods.lock.toml 2>/dev/null | keys) <(keys < mods.lock.toml)   # new mods
comm -23 <(git show <base>:mods.lock.toml 2>/dev/null | keys) <(keys < mods.lock.toml)   # removed mods
git diff <base> -- mods.lock.toml                                                          # changed hashes, ids, sides
git diff --name-status <base> -- config overrides                                          # config added, changed, removed
```

A mod whose key stays but whose `hashes` changed was updated or replaced: audit it as a new one. Skip `audit/` itself.

## 3. Audit each new or changed mod

Read its `[[mods]]` block in `mods.lock.toml` and its jar in `mods/`.

### Sources

Where the jar comes from decides whether another machine can get it:

- `[mods.modrinth]` and its `downloads`: Modrinth hosts it.
- `[mods.curseforge]`: CurseForge hosts it. Downloading it needs the CurseForge API key.
- `url` only: a direct link. Check that it points to the author's official release (their GitHub or GitLab releases, their site), and say what you found.
- None of these: the jar exists on this machine only. Warn: `modpack install` cannot fetch it elsewhere; give it a URL with `modpack add mods/<file> --url <url>`.

A mod with no `[mods.curseforge]` table may simply have been added while no CurseForge key was set. When it looks like a mod CurseForge hosts, say so: once the key is set (`modpack auth`, in the user's terminal), `modpack add mods/<file>` identifies the jar again on every platform and keeps its key and its variants.

### Known conflicts and target

- **Declared by the platform**: `curl -s https://api.modrinth.com/v2/version/<version_id>` lists `dependencies` with a `dependency_type`; an `incompatible` project the pack has (`grep -n 'project_id = "<id>"' mods.lock.toml`) is a conflict. With the key set (`test -n "$MODPACK_CURSEFORGE_API_KEY"`), `curl -s -H "x-api-key: $MODPACK_CURSEFORGE_API_KEY" https://api.curseforge.com/v1/mods/<projectID>/files/<fileID>` lists `dependencies` where `relationType` 5 means incompatible.
- **Declared by the jar**: `unzip -p mods/<file> META-INF/neoforge.mods.toml` (or `META-INF/mods.toml`): `[[dependencies.<modId>]]` with `type = "incompatible"`, and the `minecraft` and loader version ranges; `unzip -p mods/<file> fabric.mod.json`: `breaks` and `conflicts`, and `depends.minecraft`.
- **Target**: the version's `game_versions` and `loaders` on Modrinth, or the ranges in the jar, must include the `[pack]` table's `minecraft` and `loader`.
- **From your knowledge**: mods known to clash (two mods replacing the same renderer or the same system, a mod that another one already bundles). Mark these as hypotheses; the `modpack-source` skill can confirm one by reading the code.
- **Sides**: `env.client` or `env.server` set to `unsupported` while a variant holding the mod declares the opposite `side` condition. `modpack test --json` checks the static conditions; mention its failures.

### Platforms

Build every pack into a throwaway directory, `/tmp/modpack-audit-<name of the repository directory>`, which is a dry run of the publication. Each command runs in a fresh shell: write the path out in full in both.

```bash
modpack pack --out "/tmp/modpack-audit-<name of the repository directory>" --json
rm -rf "/tmp/modpack-audit-<name of the repository directory>"
```

Exit code 3 with a JSON report is expected when a platform is blocked. In the report, `blocked` lists for each variant and platform the mods that stop it, with the reason: a `.mrpack` only downloads from Modrinth, GitHub or GitLab (another mod must be embedded with its author's permission), and a CurseForge pack only takes mods hosted on CurseForge or on its approved list of third-party mods, which has no API. Warn about every blocked pair, and say which of its mods are new since the base.

## 4. Orphan configuration

A removed mod leaves its config behind. Candidates:

- with a base: the config files that belong to a mod removed since the base, and the config files added since the base;
- first audit: every file under `config/` and `overrides/*/config/`.

`.gitkeep` files only keep empty directories in git: skip them.

For each candidate, find the mod it belongs to:

1. **By name**: most files carry the mod's id: `config/<modid>.toml`, `config/<modid>-client.toml`, `config/<modid>/...`. Compare it, ignoring case, `-` and `_`, with the keys of `mods.lock.toml` and with the mod ids the jars declare (`modId` in `META-INF/neoforge.mods.toml` or `META-INF/mods.toml`, `id` in `fabric.mod.json`). For a removed mod, its key and the file name of its jar at the base (`git show <base>:mods.lock.toml`) give its ids.
2. **By history, when the name is ambiguous**: the commit that added the file, `git log --diff-filter=A --format=%H -1 -- <file>`, and the mods that commit added, `git show <commit> -- mods.lock.toml | grep '^+key = '`. A config that appeared with a mod most likely belongs to it.
3. **Neither**: some files belong to the game or the loader (`config/neoforge-*.toml`, `config/fml.toml`) or to no mod you can name. Leave them, and list the ones you could not attribute.

A file is orphan when the mod it belongs to is no longer in `mods.lock.toml`. An override is orphan as well when its variant no longer holds the mod (`overrides/<variant>/config/` of a variant whose selection drops it).

## 5. Report

Show the findings to the user, grouped by mod, then the orphan configs, then the blocked platforms. Each warning says what was found, where it comes from, and what to do about it.

Propose deleting the orphan configs, one by one or all at once. Delete only the ones the user accepts, with `git rm <file>`.

Then write `audit/last.toml` (unless this was a preview of uncommitted changes), creating `audit/` when needed:

```toml
# Written by the modpack-audit skill: the last audit of this pack.
commit = "<git rev-parse HEAD>"
date = "<today, YYYY-MM-DD>"

[[warnings]]
subject = "<mod key, config path, or variant/platform>"
kind = "<source | conflict | target | platform | config>"
message = "<one sentence: what was found and what to do>"
```

One `[[warnings]]` table per warning still open; none when the audit found nothing. Suggest committing the report together with the config deletions; do not commit without the user's go.
