---
name: modpack-curseforge-page
description: Create or update the CurseForge project page of this modpack from the versioned page in `page/`, by driving a real Chrome with the `agent-chrome` skill. Use when `modpack page push` says to run `modpack-curseforge-page`, or when asked to create the CurseForge project, or to bring its page (description, summary, logo, screenshots, links, categories, license) in line with `page/`. Not for publishing a version of the pack, which CI does by API.
---

# CurseForge project page

CurseForge has no API to manage a project, only to upload files. So this page
is pushed through the website, in the browser, from what `page/` holds. The
repository is the source of truth: the website ends up showing what `page/`
says, nothing more.

Never run this in CI. It needs a person nearby for anything that only they
can do.

## What `page/` holds

| File | CurseForge field |
|---|---|
| `page/page.toml`, `name` | Project name. Empty: the `name` in `.modpack/socle.toml`. |
| `page/page.toml`, `summary` | Summary. |
| `page/description.md` | Description (Markdown). |
| `page/icon.png` | Logo, when present. |
| `page/gallery/` and `gallery/gallery.toml` | Images: listed ones first, in their order, with title and description, then the other images by file name. |
| `page/page.toml`, `[links]` | Source, issues, wiki and Discord links, where CurseForge has a field for them. An empty link is left empty. |
| `page/page.toml`, `[curseforge]` | `project_id` (written by this skill), `categories` (main one first), `license`. |

Read all of it before opening the browser. If `summary`, `categories` or
`license` in `[curseforge]` is empty, stop and ask the person for the
missing values, then write them into `page/page.toml`.

## Steps

1. **Open the browser** with the `agent-chrome` skill, on
   `https://authors.curseforge.com/`. If the page asks to sign in, or shows
   a captcha, a two-factor prompt or terms to accept, stop: ask the person to
   do it in that window and say when it is done. Never type a password.
2. **Find the project.**
   - `[curseforge] project_id` is set: open that project in the authors
     console.
   - It is empty: look for a project with the same name first, in case it
     was created without its id being recorded. If there is one, ask the
     person whether it is this pack's project before using it.
   - None: create one. Game Minecraft, class Modpacks, then fill the fields
     below. Before the final "create" or "submit" button, show the person
     what is about to be sent and wait for their go: creating a project
     starts CurseForge's moderation.
3. **Fill the fields** from the table above. Take the description from
   `page/description.md` as is, in the Markdown mode of the editor when it
   offers one. Choose categories and license by the names in
   `[curseforge]`; when a name does not exist in CurseForge's list, stop and
   ask which one to use, then fix `page/page.toml`.
4. **Logo and images.** Upload `page/icon.png` as the logo if CurseForge
   shows another one. In the images section, make the list match
   `page/gallery/`: add the missing images, set their title and
   description, remove the ones that are not in `page/gallery/` any more
   (ask before removing one you did not upload yourself in this repository's
   history), and keep the order of `gallery.toml` when the site lets you
   order them.
5. **Save**, then **record the id**: once the project exists, read its
   project id in the authors console (the "About project" or settings area
   shows it) and write it into `[curseforge] project_id` of
   `page/page.toml`. Say that this change is to be committed.
6. **Check visually.** Open the public project page (or its preview while
   it awaits moderation), take a screenshot, and compare it to `page/`:
   name, summary, description rendering (headings, lists, links), logo,
   images and their order, links, categories, license. Fix what differs,
   then report what was done and what CurseForge still has to approve.

## When to stop and ask

Stop and hand over to the person, saying exactly what is needed, when:

- the site asks to sign in, confirm an email, solve a captcha, accept terms
  or validate anything on the account;
- a field CurseForge requires has no value in `page/`;
- an action cannot be undone (creating, submitting for review, deleting the
  project or an image the person may want to keep);
- the page looks different from what these steps describe: describe what
  you see rather than guess.
