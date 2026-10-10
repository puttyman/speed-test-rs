# Design tokens: Unloan-inspired values

Status: **to be captured** (SPEC §7.2, open question 1). Until this file is filled in, the M0
placeholder palette in `style/main.scss` stays in use.

The style (color, fonts, look and feel) is *inspired by* unloan.com.au. We don't use its name,
logo, illustrations, copy or proprietary font files. Each font is replaced by the closest
open-licensed (OFL/Apache) font, self-hosted in `public/fonts/`.

## How to capture

1. Open https://www.unloan.com.au in desktop Chrome or Firefox, then open DevTools → Console.
2. Paste the snippet below and press Enter. It prints the most-used text colors, backgrounds,
   font stacks, radii and shadows, plus any `:root` custom properties.
3. Paste the output into the tables below, or into an issue or chat. Screenshots of the home
   page, a form, and a button or card close-up help too.

```js
(() => {
  const tally = (m, k) => k && m.set(k, (m.get(k) || 0) + 1);
  const top = (m, n = 15) => [...m].sort((a, b) => b[1] - a[1]).slice(0, n);
  const color = new Map(), bg = new Map(), font = new Map(), radius = new Map(), shadow = new Map();
  for (const el of [...document.querySelectorAll("body *")].slice(0, 5000)) {
    const s = getComputedStyle(el);
    tally(color, s.color);
    if (s.backgroundColor !== "rgba(0, 0, 0, 0)") tally(bg, s.backgroundColor);
    tally(font, `${s.fontFamily} | ${s.fontWeight} | ${s.fontSize}`);
    if (s.borderRadius !== "0px") tally(radius, s.borderRadius);
    if (s.boxShadow !== "none") tally(shadow, s.boxShadow);
  }
  const rootVars = [...document.styleSheets]
    .flatMap((ss) => { try { return [...ss.cssRules]; } catch { return []; } })
    .filter((r) => r.selectorText === ":root")
    .map((r) => r.cssText).join("\n");
  const faces = [...document.fonts].map((f) => `${f.family} ${f.weight} ${f.style}`);
  console.log(JSON.stringify({ color: top(color), background: top(bg), font: top(font),
    radius: top(radius), shadow: top(shadow, 5), fontFaces: [...new Set(faces)] }, null, 1));
  console.log(rootVars.slice(0, 8000));
})();
```

## Captured values

| Token | Light | Dark | Source / notes |
|---|---|---|---|
| `--color-brand` | | | |
| `--color-brand-strong` | | | |
| `--color-brand-soft` | | | |
| `--color-bg` | | | |
| `--color-surface` | | | |
| `--color-text` | | | |
| `--color-text-muted` | | | |
| `--radius-md` / `--radius-pill` | | | |
| `--shadow-1` | | | |

## Fonts

| Role | Unloan uses | Open replacement (license) |
|---|---|---|
| Display / numbers | | |
| Body | | |
| Mono | – | JetBrains Mono (OFL) |
