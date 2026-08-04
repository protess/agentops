---
type: Runbook
title: Frontend assets — vendoring and rebuild
description: How the committed htmx and Tailwind assets are produced, and the silent failure modes to check for when regenerating them
status: stable
tags: [frontend, htmx, tailwind, vendoring, runbook]
stale_after: 2027-02-04
generated:
  by: claude-opus-5
  at: 2026-08-04
supersedes: []
---

# Frontend assets

`crates/agentops-server/static/` contains **committed static files**. There is
no Node.js in this project and no CDN at runtime (design spec, Section 14).
In the normal workflow you never rebuild anything — you run `cargo run` and
the committed assets are served as they are.

This document exists for the rare case where you do need to regenerate them.
Both procedures below have a verification step, and both verification steps
exist because the naive command **succeeds while producing nothing usable**.

## htmx — vendored, no rebuild needed

| File | Version | Size on disk |
|---|---|---|
| `static/htmx.min.js` | htmx 2.0.4 | 50,917 bytes |
| `static/sse.js` | htmx-ext-sse 2.2.2 | 8,896 bytes |

To re-download:

```bash
curl -fsSL https://unpkg.com/htmx.org@2.0.4/dist/htmx.min.js \
  -o crates/agentops-server/static/htmx.min.js
curl -fsSL https://unpkg.com/htmx-ext-sse@2.2.2/sse.js \
  -o crates/agentops-server/static/sse.js
```

**Always verify afterwards.** A common failure mode is a 404 HTML page being
saved under a `.js` name — `curl` exits 0 and you get a file that is not
JavaScript:

```bash
wc -c crates/agentops-server/static/*.js
head -c 120 crates/agentops-server/static/htmx.min.js
```

If a file is not tens of kilobytes, or if it starts with `<!DOCTYPE`, the
download failed.

## Tailwind CSS — standalone CLI, zero Node dependencies

**If the `tailwindcss` binary is not on the system, do not build. Use the
committed `static/app.css` as-is.** Installing Tailwind through Node violates
the design spec (Section 14).

### The version trap

The Tailwind CLI's syntax changed between v3 and v4, and `releases/latest`
now resolves to v4. Measured differences:

- The v4 CLI **has no `-c` / `--config` flag.** It is not in
  `tailwindcss --help`, and passing it produces no error — it is **silently
  ignored**, exiting 0 even when it points at a file that does not exist.
- Building with v3 syntax (`@tailwind base; @tailwind components;
  @tailwind utilities;` plus the CLI's `-c`) **succeeds without any error but
  never reads the `content` array in `tailwind.config.js`.** Every utility
  class the templates use — `text-xl`, `mb-4`, `bg-neutral-950` — is absent
  from the output CSS. This is the same shape as the defect class this
  project keeps finding in its own tests: a step that reports success and
  verifies nothing. Here it appeared in an infrastructure script.
- v4 requires the config to be loaded explicitly from the CSS side with
  `@config`, and uses `@import "tailwindcss";` instead of `@tailwind ...`.

That is why `tailwind.src.css` uses v4 syntax:

```css
@config "./tailwind.config.js";
@import "tailwindcss";
```

The `content: ["./templates/**/*.html"]` entry in `tailwind.config.js` is
still valid and still honored — verified by confirming that `text-xl`,
`bg-neutral-950`, `space-y-2`, and `hover:text-white` all appear in the
output.

### Rebuilding

**The binary is fetched by pinned tag, not by `releases/latest`. This is
deliberate — do not change it back.** Everything above is what happened when
it was `latest`.

Pinning alone is not enough, though: a future major version can reintroduce
the same silent failure through a different flag. So **the build command
verifies its own output**:

```bash
curl -fsSL -o /usr/local/bin/tailwindcss \
  https://github.com/tailwindlabs/tailwindcss/releases/download/v4.3.3/tailwindcss-macos-arm64
chmod +x /usr/local/bin/tailwindcss

cd crates/agentops-server && tailwindcss -i tailwind.src.css -o static/app.css --minify
grep -q '\.text-xl{' static/app.css \
  || { echo "build emitted no utility classes — the CLI syntax probably changed"; exit 1; }
```

(The URL is the macOS arm64 asset; other platforms differ only in the asset
name.)

`text-xl` is the class used by the `<h1>` in `incidents.html`, which makes it
a stable canary — it is unlikely to disappear during ordinary template edits.
When that `grep` fails, it converts the silent success of the v3/v4 mismatch
into a loud failure.
