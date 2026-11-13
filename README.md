# modpack-cli

`modpack`, the CLI of the Minecraft modpack socle: it creates a modpack repository, keeps its socle up to date, prepares a clone on a machine, feeds the pack's Modrinth instance and builds the packs. The design lives in `payangar-studio/environment`, `docs/design/00-cadrage.md` and `docs/design/01-socle-modpack.md`.

## Install

On a machine set up with chezmoi, the catalogue installs `modpack` from this repository's releases. By hand, on Linux x86_64:

```sh
version=0.1.0
curl -LO "https://github.com/payangar-studio/modpack-cli/releases/download/v$version/modpack-$version-x86_64-unknown-linux-musl.tar.gz"
curl -LO "https://github.com/payangar-studio/modpack-cli/releases/download/v$version/modpack-$version-x86_64-unknown-linux-musl.tar.gz.sha256"
sha256sum -c "modpack-$version-x86_64-unknown-linux-musl.tar.gz.sha256"
tar -xzf "modpack-$version-x86_64-unknown-linux-musl.tar.gz" -C ~/.local/bin
```

From source, with a Rust toolchain: `cargo install --git https://github.com/payangar-studio/modpack-cli --locked`.

The Modrinth App must be installed for `modpack setup` to create the pack's instance.

## Commands

| Command | What it does |
|---|---|
| `modpack init [dir] --name --mc-version --loader --loader-version --author` | Creates a modpack repository: socle, layout, default mode. Asks for each value not given. |
| `modpack setup [--no-launch]` | Prepares a clone on this machine: enables the git hooks, downloads the jars, registers the instance in the Modrinth App (one click to confirm the import), loads the default mode. Safe to run again. |
| `modpack update [--force]` | Brings the repository's socle to this CLI's version. Stops on socle files edited by hand, unless `--force`. |
| `modpack add <mod> [--version] [--mode] [--with-deps] [--url]` | Adds a mod from Modrinth or CurseForge (slug, id or URL) or from a local jar, and updates the manifest. Asks for the version when several fit. |
| `modpack remove <key>` | Removes a mod from the repository and from the modes that list it. The only way a mod leaves the pack. |
| `modpack manifest [--check]` | Updates `mods.lock.toml` from `mods/`: adds the new jars, rebuilds the entries whose jar changed. A jar missing from `mods/` keeps its entry, with a warning (not installed here is not removed). A jar `modpack install` downloaded that the manifest no longer lists is never added back. `--check` fails when it is stale. Both then check `modes.toml` against the manifest (unknown keys, unknown conditions). Run by the `pre-commit` hook. |
| `modpack install [--force]` | Downloads the missing jars into `mods/`, removes the ones it downloaded that the manifest no longer lists (recorded in `.modpack/local/installed.toml`), then reloads the mode loaded in the instance. Run by the `post-merge` and `post-checkout` hooks. |
| `modpack mode list` | Lists the modes and marks the loaded one. |
| `modpack mode load <mode> [--force]` | Loads a mode into the instance: its jars, the base config and its overrides; removes the config files written for the previous mode. Refuses while Minecraft runs; stops on files changed in the instance, unless `--force`. |
| `modpack status [--json]` | Loaded mode, drift, jars unknown to the manifest, state of the manifest. The JSON format is in [`docs/status.md`](docs/status.md). |
| `modpack test [mode] [--condition] [--json]` | Checks the conditions of a mode, or of all modes. |
| `modpack pack [mode] [--platform] [--out] [--pack-version] [--json]` | Builds the `.mrpack` and the CurseForge zip of a mode, or of all modes, into `dist/`. |
| `modpack page push [--platform]` | Pushes `page/` to the platforms that have an API (Modrinth) and names the skill to run for the others (CurseForge). Never in CI. |
| `modpack auth [secret] [--status] [--remove <secret>]` | Stores the platform secrets in the system keyring, with masked input. |

Every command runs without a terminal (AI agent, git hook, CI): it then asks nothing, and a missing value is an error that names the argument to pass. Errors say what happened and what to do. Exit codes: `0` success, `1` internal failure, `2` usage error (missing or wrong argument, not in a modpack repository), `3` refused: overridable with `--force`, or a check that did not pass (a failed `modpack test` condition, a platform `modpack pack` cannot build for). `-v` and `-q` set how much is logged on stderr.

## Secrets on a machine

The CLI reads each secret from its environment variable first, then from the system keyring (Secret Service on Linux, Credential Manager on Windows), where `modpack auth` stores it:

| Secret | Variable | Needed by |
|---|---|---|
| Modrinth personal access token | `MODPACK_MODRINTH_TOKEN` | `modpack page push` |
| CurseForge upload token | `MODPACK_CURSEFORGE_UPLOAD_TOKEN` | publication, in CI only |
| CurseForge Core API key | `MODPACK_CURSEFORGE_API_KEY` | identifying and downloading the mods hosted on CurseForge |

Looking a jar up on Modrinth needs no secret. Run `modpack auth` in your own terminal: values are never passed as arguments nor shown.

## Publication

A modpack repository publishes its packs from CI, never from a machine. The socle lays down `.github/workflows/modpack-publish.yml`, which calls the reusable workflow of this repository, `.github/workflows/publish-modpack.yml`, pinned to the release of `modpack` that wrote the socle. `modpack update` moves the pin.

Pushing a `v*` tag on `main` of the pack:

1. checks that the tagged commit is on `origin/main`, and stops otherwise (a guard against a tag on a work branch, not a security boundary);
2. installs `modpack` from this repository's release;
3. runs `modpack pack` for each published mode, which builds from the manifest alone: no jar is downloaded;
4. reads the secrets from the vault (Infisical) by OIDC, as the identity `ci-minecraft`;
5. creates a version on Modrinth and uploads a file to CurseForge for each pack. A platform that fails does not stop the other.

What each pack repository needs, once:

- repository variables (Settings, Secrets and variables, Actions, Variables): `INFISICAL_URL`, the vault's URL, and `INFISICAL_IDENTITY_ID`, the id of `ci-minecraft`. Neither is a secret; an organization variable does not reach a private repository on GitHub Free. `MODPACK_PUBLISH_MODES` optionally lists the modes to publish, separated by spaces (all modes when unset);
- the project ids in `page/page.toml`: `[modrinth] project_id`, written by `modpack page push`, and `[curseforge] project_id`, written by the `modpack-curseforge-page` skill.

The secrets live in the vault project `minecraft`, environment `prod`, folder `/app`, under the names of the variables the CLI reads: `MODPACK_MODRINTH_TOKEN` and `MODPACK_CURSEFORGE_UPLOAD_TOKEN`. Nothing goes into GitHub secrets.

What gets published:

- version `1.2.0` from tag `v1.2.0`. A suffix makes a prerelease: `-alpha…` is an alpha, any other suffix a beta;
- the default mode as the version itself, every other mode as `<version>+<mode>` on Modrinth and `<name> <version> (<mode>)` on CurseForge;
- the changelog is `changelogs/<version>.md` when it exists, else the tag's message.

Running the workflow by hand (Actions, Publish, Run workflow) from the tag publishes again, to one platform only if needed, or with `dry-run` builds the packs and keeps them as a workflow artifact without reading the vault or publishing. A dry run also works from a branch, as version `0.0.0-dev`.

## Development

`AGENTS.md` describes the layout, the conventions and how to run the tests. Pushing a `v*` tag on this repository publishes a release with the static Linux binary.

## License

MIT, see [`LICENSE`](LICENSE).
