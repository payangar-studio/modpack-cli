# .modpack

This directory belongs to the socle of the modpack, managed by `modpack-cli`.

## Who owns what

- **Socle files**: everything in `.modpack/` but `local/`, the skills in `.claude/skills/modpack-*/` and the workflows in `.github/workflows/modpack-*.yml`. `modpack update` rewrites them.
- **Mixed files**: `AGENTS.md` and `.gitignore`. The socle owns the block between its `modpack:begin` and `modpack:end` markers; the rest of the file belongs to the pack.
- **Machine state**: `.modpack/local/`, ignored by git. `installed.toml` records the jars `modpack install` downloaded into `mods/`, so that it removes them once the manifest drops them and `modpack manifest` never adds them back.
- **Pack content**: `mods.lock.toml`, `mods/`, `config/`, `modes.toml`, `overrides/` and the page. The socle never touches them after `modpack init`.

## Updating

`modpack update` brings the socle to the version of the installed CLI. `socle.lock` records the hash of everything the socle wrote: if a socle file or block was edited by hand, the update stops and lists it, and `modpack update --force` overwrites it. Keep your own changes outside the socle's files.

`socle.toml` records the socle version and the values given to `modpack init`. A CLI older than the repository's socle refuses to work in it; a newer one warns and suggests `modpack update`.
