# Design tokens: Unloan-inspired values

Status: **to be captured** (SPEC §7.2, open question 1). Until this file is filled in, the M0
placeholder palette in `style/main.scss` stays in use.

The style (color, fonts, look and feel) is *inspired by* unloan.com.au. We don't use its name,
logo, illustrations, copy or proprietary font files. Each font is replaced by the closest
open-licensed (OFL/Apache) font, self-hosted in `public/fonts/`.

## How to capture

1. Open https://www.unloan.com.au in desktop Chrome or Firefox, then open DevTools → Console.
2. Paste the snippet below and press Enter. It returns (and copies to the clipboard) the most-used colors,
   backgrounds, borders, font stacks, radii, shadows, transitions and animations, sample button
   and heading styles, `@keyframes`, and any `:root` custom properties.
3. Paste the output into the tables below, or into an issue or chat. Screenshots of the home
   page, a form, and a button or card close-up help too.

```js
(() => {
  const tally = (m, k) => k && m.set(k, (m.get(k) || 0) + 1);
  const top = (m, n = 20) => [...m].sort((a, b) => b[1] - a[1]).slice(0, n);
  const t = Object.fromEntries(["color", "background", "border", "font", "radius", "shadow", "transition", "animation", "gap"].map((k) => [k, new Map()]));
  for (const el of [...document.querySelectorAll("body *")].slice(0, 8000)) {
    const s = getComputedStyle(el);
    if (s.display === "none") continue;
    tally(t.color, s.color);
    if (s.backgroundColor !== "rgba(0, 0, 0, 0)") tally(t.background, s.backgroundColor);
    if (s.backgroundImage !== "none") tally(t.background, s.backgroundImage.slice(0, 200));
    if (s.borderTopWidth !== "0px") tally(t.border, `${s.borderTopWidth} ${s.borderTopStyle} ${s.borderTopColor}`);
    tally(t.font, `${s.fontFamily} | ${s.fontWeight} | ${s.fontSize}/${s.lineHeight} | ls ${s.letterSpacing}`);
    if (s.borderRadius !== "0px") tally(t.radius, s.borderRadius);
    if (s.boxShadow !== "none") tally(t.shadow, s.boxShadow);
    if (s.transitionDuration !== "0s") tally(t.transition, `${s.transitionProperty} ${s.transitionDuration} ${s.transitionTimingFunction}`);
    if (s.animationName !== "none") tally(t.animation, `${s.animationName} ${s.animationDuration} ${s.animationTimingFunction}`);
    if (s.gap && s.gap !== "normal") tally(t.gap, s.gap);
  }
  const rules = [...document.styleSheets].flatMap((ss) => { try { return [...ss.cssRules]; } catch { return []; } });
  const sample = (sel) => [...document.querySelectorAll(sel)].slice(0, 4).map((e) => {
    const s = getComputedStyle(e);
    return { text: e.textContent.trim().slice(0, 30), bg: s.backgroundColor, color: s.color, radius: s.borderRadius,
      padding: s.padding, font: `${s.fontWeight} ${s.fontSize} ${s.fontFamily}`, shadow: s.boxShadow };
  });
  const out = {
    url: location.href, viewport: `${innerWidth}x${innerHeight}`,
    ...Object.fromEntries(Object.entries(t).map(([k, v]) => [k, top(v)])),
    fontFaces: [...new Set([...document.fonts].map((f) => `${f.family} ${f.weight} ${f.style}`))],
    buttons: sample('button, a[class*="button" i], a[class*="btn" i], [role="button"]'),
    headings: sample("h1, h2, h3"),
    keyframes: rules.filter((r) => r.type === CSSRule.KEYFRAMES_RULE).slice(0, 15).map((r) => r.cssText.slice(0, 400)),
    rootVars: rules.filter((r) => r.selectorText && /(^|,)\s*:root/.test(r.selectorText)).map((r) => r.cssText).join("\n").slice(0, 15000),
  };
  const json = JSON.stringify(out, null, 1);
  try { copy(json); } catch {}
  return json;
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
