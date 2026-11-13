# modpack-cli

CLI of the Minecraft modpack socle. Design: `payangar-studio/environment`, branch `design`.

## Publication

A modpack repository publishes its packs from CI, never from a machine. The socle lays down `.github/workflows/modpack-publish.yml`, which calls the reusable workflow of this repository, `.github/workflows/publish-modpack.yml`, pinned to the release of `modpack` that wrote the socle. `modpack update` moves the pin.

Pushing a `v*` tag on `main` of the pack:

1. checks that the tagged commit is on `origin/main`, and stops otherwise (a guard against a tag on a work branch, not a security boundary);
2. installs `modpack` from this repository's release;
3. runs `modpack install`, `modpack manifest --check`, then `modpack pack` for each published mode;
4. reads the secrets from the vault (Infisical) by OIDC, as the identity `ci-minecraft`;
5. creates a version on Modrinth and uploads a file to CurseForge for each pack. A platform that fails does not stop the other.

What each pack repository needs, once:

- repository variables (Settings, Secrets and variables, Actions, Variables): `INFISICAL_URL`, the vault's URL, and `INFISICAL_IDENTITY_ID`, the id of `ci-minecraft`. Neither is a secret; an organization variable does not reach a private repository on GitHub Free. `MODPACK_PUBLISH_MODES` optionally lists the modes to publish, separated by spaces (all modes when unset);
- the project ids in `page/page.toml`: `[modrinth] project_id`, written by `modpack page push`, and `[curseforge] project_id`, written by the `modpack-curseforge-page` skill.

The secrets live in the vault project `minecraft`, environment `prod`, folder `/app`, under the names of the variables the CLI reads: `MODPACK_MODRINTH_TOKEN`, `MODPACK_CURSEFORGE_UPLOAD_TOKEN`, and `MODPACK_CURSEFORGE_API_KEY` (used by `modpack install` for the mods hosted only on CurseForge). Nothing goes into GitHub secrets.

What gets published:

- version `1.2.0` from tag `v1.2.0`. A suffix makes a prerelease: `-alpha…` is an alpha, any other suffix a beta;
- the default mode as the version itself, every other mode as `<version>+<mode>` on Modrinth and `<name> <version> (<mode>)` on CurseForge;
- the changelog is `changelogs/<version>.md` when it exists, else the tag's message.

Running the workflow by hand (Actions, Publish, Run workflow) from the tag publishes again, to one platform only if needed, or with `dry-run` builds the packs and keeps them as a workflow artifact without reading the vault or publishing. A dry run also works from a branch, as version `0.0.0-dev`. A dry run cannot download the mods hosted only on CurseForge, since the API key is in the vault.
