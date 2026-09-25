---
title: Install button for your mod page
---

# Install button for your mod page

If your mod site or personal page isn't (yet) supported by the [DDMM browser extension](../using/one-click-install.md),
you can still offer a one-click install by linking directly to your download with the `ddmm://install` scheme —
no extension required.

## The link format

```
ddmm://install?url=<percent-encoded https URL>
```

`url` must be a **percent-encoded `https://` URL** — either a direct download link, or a mod page URL that DDMM
already knows how to handle (an AyakaMods or Nexus Mods page, for example, opens the same
[browser handoff](../using/mod-sites.md#how-the-browser-handoff-works) the in-app **Add URL** button uses). Any
other scheme (`http://`, `file:`, `javascript:`, ...) is rejected outright, and clicking the link always shows the
user a confirmation first — there's no way to skip it, by design.

## Example

For a direct download at `https://example.com/mods/cool-mod-v1.zip`:

```html
<a href="ddmm://install?url=https%3A%2F%2Fexample.com%2Fmods%2Fcool-mod-v1.zip">
    Install with DDMM
</a>
```

If DDMM isn't installed, the link simply does nothing (the browser doesn't recognize the `ddmm://` scheme) — it
doesn't error or redirect anywhere, so it's safe to show unconditionally. There's no reliable, cross-browser way
to detect whether the scheme is registered before the click, so most mod pages pair the button with a plain link
to the [download page](../getting-started/download.md) for players who don't have DDMM yet.

## What the user sees

Clicking the link brings DDMM to the front (starting it first if it wasn't already running) and shows:

> **Install This Mod?** — A link wants to install a mod from &lt;site&gt;. Install?

Confirming runs the same install path as pasting the URL into **Add URL** yourself — including, for a login-gated
site, opening it in the user's browser so their own session handles the download.

## Percent-encoding the URL

Encode the target URL with your language's standard URL-encoding function before building the `href` — for
example, JavaScript's `encodeURIComponent`:

```js
const target = "https://example.com/mods/cool-mod-v1.zip";
const href = `ddmm://install?url=${encodeURIComponent(target)}`;
```

An unencoded or malformed `url` parameter is ignored (the link does nothing) rather than causing an error.
