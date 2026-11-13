# Local mods

A local mod is a jar no platform serves: an abandoned mod, a build of a mod under development, a file received from its author. Its jar lives in `mods-local/`, at the root of the pack repository, and git tracks it. It is published only once approved.

## Adding one

Drop the jar into `mods-local/`. `modpack manifest`, which the `pre-commit` hook runs, adds it to `mods.lock.toml`, and the hook adds the jars of `mods-local/` to the commit. `modpack add mods-local/<jar>` does the same at once.

The instance's `mods/` holds every jar by its file name, so a jar of `mods-local/` cannot share its file name with a jar of `mods/` or of another entry: `modpack manifest` and `modpack add` refuse it, naming both files. Rename one of them.

The entry has source `local`, its path in `mods-local/`, its hashes and size, and no download URL. No platform is asked about it, so a commit works offline. Its sides are `required` on both, as for any jar no platform describes. It stays in the one manifest that modes, `modpack test` and `modpack pack` read: modes name it by its key like any other mod.

`modpack install` never downloads a local mod, git holds it; it reports the jar as failed when it is missing from `mods-local/`. `modpack mode load` (and the reload after `install`) copies it from `mods-local/` into the instance's `mods/`.

A changed jar is rebuilt by `modpack manifest` under the same key, and loses its approval. `modpack remove <key>` takes the mod out of the pack and its jar out of git (`git rm`).

## Replacing it once published

When the mod reaches Modrinth or CurseForge, `modpack add <slug>` replaces the local entry when its key is the slug of the project the platform resolves; a key that only matches the `<slug>` given is not enough. The new entry keeps the key, so modes need no change, and once the manifest is saved the jar leaves `mods-local/` and git. A jar git does not hold committed (untracked, staged only, or changed since the last commit) is never deleted: `modpack add` stops with exit code 3, unless `--force`. Commit the result.

## Approval

`modpack approve <key>` records in the manifest, as `approved`, the sha512 of the jar it approves. Approving is the user's review of the jar: its license allows redistribution, and its content is what it claims. Commit `mods.lock.toml` afterwards.

The approval holds while the jar keeps that hash. A new jar, even under the same name, is not approved until `modpack approve` runs again.

`modpack status` lists the local mods and says which are not approved. Neither it nor `modpack pack` ever blocks on them.

## Packaging

`modpack pack` embeds each local mod of a mode in the pack's overrides, as `overrides/mods/<jar>`, for every platform, and warns about each one, approved or not. It stops only when the jar in `mods-local/` no longer matches the manifest: run `modpack manifest`, then approve the new jar.

A CurseForge pack accepts an embedded jar only if it is on CurseForge's approved list of third-party mods. That list has no API: the warning says to check it by hand.

## Publication

The publish workflow runs `modpack approve --check` before reading the vault. While a local mod has no approval matching its jar, it fails, naming each mod and the `modpack approve <key>` to run. A dry run publishes nothing and only warns.
