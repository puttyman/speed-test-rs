# Design tokens: Unloan-inspired values

Status: **captured 2026-10-10** from the public pages of unloan.com.au (Melbourne, read-only;
no log-in, no form input). Raw script output is in [`raw/`](raw/).

The style (color, fonts, look and feel) is *inspired by* unloan.com.au. We don't use its name,
logo, illustrations, copy or proprietary font files. Each font is replaced by the closest
open-licensed (OFL/Apache) font, self-hosted in `public/fonts/`.

## What was captured

| Page | URL | Stack | Desktop 1440 | Mobile 390 |
|---|---|---|---|---|
| Home | `/` | Webflow | ✓ | ✓ |
| Product / rates | `/home-loan` | Next.js + Tailwind v4 | ✓ | ✓ |
| FAQ | `/faqs` | Webflow | ✓ | ✓ |
| Calculator (inputs) | `/calculators/repayments` | Next.js + Tailwind v4 | ✓ | ✓ |
| Get started, first screen | `/get-started` | Next.js + Tailwind v4 | ✓ | ✓ |

**Dark theme: none.** Emulating `prefers-color-scheme: dark` leaves every page unchanged (body
stays `#ffffff`, text `#29292b`), and no readable stylesheet has a `prefers-color-scheme`
rule. Every "Dark value" below is therefore *not observed*. Where the light theme has dark
sections, those colors are listed in [Dark-section hints](#dark-section-hints) as a starting
point; they are not a dark theme.

**Two stacks.** Marketing pages are Webflow (`--brand--*`, `--colour-text--*` custom
properties). The product, calculator and get-started pages are a Next.js app on Tailwind v4
whose theme variables (`--color-neutral-*`, `--color-green-*` …) were read from
`getComputedStyle(document.documentElement)` because its CSS is cross-origin
([`raw/home-loan_root-custom-properties.json`](raw/home-loan_root-custom-properties.json)).
The two agree on almost every value.

## 1. Token table

Colors converted from computed `rgb()` to hex. "Var" names the site's own custom property
where one exists.

| Token | Light value | Dark value | Where observed |
|---|---|---|---|
| `--color-brand` | `#101010` | not observed | Primary button bg (`up_btn-primary-lg`, `up_btn_black`), nav "Get started" pill, h1–h3 text. Var `--colour-text--text-primary-black`, `--color-neutral-1000`. The brand is effectively **monochrome black**; there is no blue/green "brand hue". |
| `--color-brand-strong` | `#000000` | not observed | 2px border on the primary button; default text on Next pages. *Interpretation:* Unloan's hover goes **lighter** (to `#353535` / `#676767`), not darker, so "strong" here is the resting edge, not a hover. |
| `--color-brand-soft` | `#efefef` | not observed | Segmented-control track, stroke-button hover/active bg, account-menu button. Var `--brand--background-grey-secondary`, `--color-neutral-200`. |
| `--color-on-brand` | `#ffffff` | not observed | Text on primary/black buttons and dark cards. |
| accent (extra) | `#f2ff00` | not observed | Var `--brand--accent-colour`, `--color-yellow-lime`. Defined on both stacks; seen as text color on 1 element on Webflow pages (element not located). Use sparingly. |
| accent gradient (extra) | `#ff7bad → #ffba2f` | not observed | FAQ category labels: `linear-gradient(to right, #ff7bad 50%, #ffba2f)` at `background-size: 200%`, clipped to text, swept on hover/active. Variant: `#ff7bad 0%, #ff869d 25%, #ffc940 50%, #232323 50%`. Vars `--unloan-pink-2`, `--unloan-yellow-1/2` (marked deleted in Webflow, still served). |
| `--color-bg` | `#ffffff` | not observed | Page background, all pages. |
| `--color-surface` | `#f7f7f7` | not observed | Grey cards (`up_card--xsmall`, `up_card-bg--grey`), sticky grey section, footer, calculator result card, secondary CTA. Var `--brand--background-grey`, `--color-neutral-100`. |
| `--color-surface-raised` | `#ffffff` + `--shadow-1` | not observed | Log-in pill, floating widgets; testimonial cards are white with a 1px `#cccccc` border instead of a shadow. Nav is `rgba(255,255,255,0.9)` + `backdrop-filter: blur(20px)`. |
| `--color-border` | `#cccccc` | not observed | Most common 1px border on home (48 elements): testimonial cards, calculator link tabs. Also `#d1d1d1` (`--border-gray`, FAQ dividers), `#dedede` (`--color-neutral-300`, product-page rules), `#e2e8f0` (input rest border), `#888888` (get-started choice cards). |
| `--color-text` | `#29292b` (body) / `#101010` (headings) | not observed | Body copy on Webflow pages is `#29292b` (Var `--unloan-black`); headings `#101010`. Next pages use `#000000` body. |
| `--color-text-muted` | `#6d6b6d` | not observed | Secondary copy on Webflow (`--colour-text--text-primary-grey`); Next pages use `#676767` (`--colour-text--text-secondary-grey`, `--color-neutral-800`). Tertiary `#dedee0`. |
| status: success | `#007d1b` (strong `#005011`, soft `#eff3f0`) | not observed | Tailwind theme `--color-green-400/500/100`. Defined, not seen rendered on the captured pages. |
| status: warning | `#ff9500` (strong `#7d4900`, soft `#fff4e7`) | not observed | `--color-orange-400/500`, `--color-brellow-100`. Also `--color-yellow-300 #ffd979`, `--color-yellow-500 #977800`. Defined, not seen rendered. |
| status: error | `#d70015` (strong `#87000d`, soft `#fbeff0`) | not observed | `--utility--error`, `--color-red-400/500/100`; `--color-red-300 #e17d7f`. `#d70015` is the bg of 5 elements on Webflow pages (likely hidden validation chips). |
| status: info / link | `#0066cc` (strong `#004284`) | not observed | All text links and footer links (`--utility--link-text`, `--color-blue-400`); `--color-blue-500`. Old nav hover blue `#2997ff`. |
| `--radius-sm` | `6px` (inner) / `8px` (outer) | — | Inputs and segmented options 6px (`rounded-md`); nav-link hover chips, segmented track, slider track 8px. Var `--radius: .5rem`. |
| `--radius-md` | `12px`–`16px` | — | Next cards and choice cards 12px (`rounded-xl`, `--border-corenr-radius-md-card: .75rem`); Webflow small cards and testimonials 16px (`--border-corner-radius-1: 1rem`). Most common radius on home: 16px (59×). |
| `--radius-lg` | `24px` | — | Large feature cards (`up_card-bg--grey`, `up_card-bg--dark`, 374×450). `--radius-3xl: 1.5rem`. |
| `--radius-pill` | `32px` (buttons) / `9999px` | — | Every CTA uses `border-radius: 32px` (`--border-corner-radius-2: 2rem`); round icon buttons use `50%` / `rounded-full`. FAQ search pill 50px. |
| `--shadow-1` | `0 3px 40px 0 rgba(0,0,0,0.07)` | — | Log-in pill (`shadow-content-card`) and floating widgets. Hover state: `0 6px 32px 0 rgba(0,0,0,0.12)`. |
| `--shadow-2` | `0 25px 50px -12px rgba(0,0,0,0.25)` | — | Floating calculator tooltip card (Tailwind `shadow-2xl`). Also seen: `0 5px 40px 0 rgba(9,14,21,0.16)` (1 element, source not located; possibly the chat launcher). Segmented thumb: `0 1px 2px rgba(0,0,0,0.05)`. |
| base spacing unit | `4px` | — | Tailwind `--spacing: .25rem`. Webflow gaps cluster on 8 / 12 / 16 / 24 / 64. Sections pad `96px 16px` desktop, `64px 16px` mobile; container max 1170px (`--max-width-container`), nav container 1202px. |
| motion: fast | `150ms cubic-bezier(0.4, 0, 0.2, 1)` | — | Next pages colour and shadow transitions (`--default-transition-*`). |
| motion: base | `200ms ease` | — | Webflow button bg/border transitions (most common, 37×). Next pages 300ms `cubic-bezier(0.4,0,0.2,1)`. |
| motion: control | `350ms ease` | — | Segmented-control thumb `transform` and label `color`; FAQ accordion `height`. |
| motion: slow | `600ms` | — | Carousel slide fade `opacity 0.6s cubic-bezier(0.25, 1, 0.5, 1)`; `bottom-up` reveal `0.6s ease-in-out` (stagger delay 0.3s). |
| easing curves | see note | — | `ease`; `cubic-bezier(0.4,0,0.2,1)` (in-out); `cubic-bezier(0,0,0.2,1)` (out); `cubic-bezier(0.25,1,0.5,1)` (soft out, carousel); `ease-in-out` (gradient sweep 275ms). |

### Dark-section hints

Not a dark theme; these are the dark surfaces used inside the light theme.

| Use | Value | Source |
|---|---|---|
| Dark card / section bg | `#171d26` | `--brand--background-dark`, `up_card-bg--dark` |
| Dark secondary | `#222222` | `--brand--background-dark-secondary` |
| Near-black accent | `#1d1d1f` | `--unloan-black-accent` |
| Mid grey | `#404042` | `--unloan-mid` |
| Text / ghost button on dark | `#ffffff`, border 1.5px `#ffffff` | `up_btn_card--white` |

## 2. Fonts

| Role | Unloan font | Licence type | Closest open replacement |
|---|---|---|---|
| Display headings (h1, hero h2) | **UnloanRaw** Medium 500 (Regular 400 also loaded) | Custom brand font, self-hosted as `UnloanRaw-*.woff2`; not on Google Fonts; no licence published on the site. **Treat as proprietary.** | **IBM Plex Mono 500** (OFL). Alternative: Martian Mono (OFL). *Visual judgement:* UnloanRaw reads as a monospaced-feeling grotesque with curved `l`/`t` tails; Plex Mono matches that best. |
| Big numbers (rate) | **UnloanRaw Numbers** (rendered at 700) | Custom, proprietary (same family) | **Inter 700** with `font-variant-numeric: tabular-nums` and tight tracking (−2.5%), or Geist 700 (OFL). *Guess* from appearance: heavy, compact grotesque digits. |
| Section headings, UI, body | **Inter** 400 / 500 / 600 / 700 (Webflow also loads 900) | **Open** (OFL), Google Fonts | Inter itself (self-host). |
| Not observed in use | UnloanMarkup 400 | Custom, proprietary | – (loaded on all pages; no element seen using it) |
| Not observed in use | SF Compact Display/Text 500, Google Sans 500 | Proprietary (Apple / Google) | – (loaded on Webflow pages, probably app-store badges) |
| Chat widget only | Proxima Nova (Intercom) | Commercial | – (third-party, ignore) |
| Mono (our need) | – | – | JetBrains Mono (OFL) |

### Type scale

Letter-spacing on Inter headings is consistently **−3% of font size** (64→−1.92px,
32→−0.96px, 24→−0.72px, 16→−0.48px). Body tracking is `normal` or −0.48px at 16px.

| Role | Desktop (size/line, tracking) | Mobile 390 |
|---|---|---|
| h1 display (UnloanRaw 500) | 80/88, `normal` or −1.92px | 48/56, −1.44px |
| h2 display (UnloanRaw 500) | 72/80 | 40/48, −1.2px |
| h1 Inter (Next pages, 600) | 72/80, −2.16px | 40/48, −1.2px |
| h2 Inter 600 | 64/76, −1.92px (product page 48/60, −1.2px) | 36/44, −1.08px (product 32/40, −0.8px) |
| h3 Inter 600 | 32/40, −0.96px | 28/36, −0.84px |
| Subheading Inter 600 | 24/32, −0.72px | 20/28, −0.6px |
| Body Inter 400 | 14/20 (Webflow), 16/24 −0.48px | 16/24, 17/24 −0.48px |
| Rate digits (UnloanRaw Numbers 700) | 150/150; `%` 105/82.5 | 56/56 |
| Product-page numbers | 64/64, −1.6px | 44/44, −1.1px |
| Button labels Inter 600 | 16/24 (lg), 14/20 −0.42px (md) | 16/22.4 |

## 3. Component specs

States come from the site's own `:hover` / `:active` rules (Webflow) and Tailwind `hover:` /
`active:` / `focus-within:` classes (Next). **Focus:** no custom `:focus-visible` style on
Webflow buttons or links; keyboard focus shows the browser's default ring. Next inputs use a
1px black ring (below). We should add our own visible focus ring.

### Primary button (`up_btn-primary-lg`)
- Rest: bg `#101010`, text `#ffffff`, border 2px solid `#000000`, radius 32px, padding 16px 48px
  (height 60px), Inter 600 16/24, no shadow. Mobile: full width (358px), 16/22.4.
- Hover: bg `#676767`.
- Active: bg `#676767`, outline 2px solid `#888888`, outline-offset −2px.
- Transition: `border 0.2s ease, background-color 0.2s ease`.
- Medium variant (`up_btn_black`): padding 16px 48px, height 52px, Inter 600 14/20, no border;
  hover `#353535`; active as above. Nav "Get started" pill: height 40px, radius 32px; hover
  `#272729` (Webflow) / `#353535` (Next `hover:bg-neutral-900`).

### Secondary button (`up_btn-stroke-lg`)
- Rest: transparent, text `#101010`, border 2px solid `#101010`, radius 32px, padding
  16px 24px (height 60px), Inter 600 16/24.
- Hover: bg `#efefef`, border `#888888`.
- Active: bg `#efefef`, border `#353535`.
- Medium (`up_btn-stroke-md`): border 1.5px, padding 12.8px 32px, Inter 600 14/20, −0.42px.

### Soft button (Next, "Get started" in page footer CTA)
- bg `#f7f7f7`, text `#000000`, radius 32px, padding 12px 40px (height 48px), Inter 600 16/24
  −0.48px. Hover bg `#dedede`. Transition colors 150ms `cubic-bezier(0.4,0,0.2,1)`.

### Ghost on dark (`up_btn_card--white`)
- Transparent, text `#ffffff`, border 1.5px `#ffffff`, radius 32px, padding 12px 24px, Inter 600
  14/20. Hover bg `rgba(87,87,87,0.08)`, border `#888888`.

### Icon buttons
- `up_btn-quick-link`: 40×40 circle, bg `#ffffff`; hover `#dedede`; active `#a2a2a2`.
- Next round button: 48×48 circle, white, border 2px `#cccccc`; hover `#f7f7f7`; active `#dedede`.

### Segmented control (Live-in / Invest)
- Track: bg `#efefef`, radius 8px, padding 2px.
- Option: 160px wide, padding 4px 8px, radius 6px, Inter 600 14/20, −0.42px, text `#29292b`.
- Thumb: `::before`, bg `#ffffff`, radius 6px, shadow `0 1px 2px rgba(0,0,0,0.05)`, slides with
  `transform 0.35s`; labels `color 0.35s`. Good fit for our transport/units toggle.

### Card
- Grey card (`up_card--xsmall`): bg `#f7f7f7`, radius 16px, padding 24px (mobile 24px 16px), no
  border, no shadow.
- Feature card: radius 24px, bg `#f7f7f7` or `#171d26`, 374×450 desktop.
- Testimonial: bg `#ffffff`, border 1px `#cccccc`, radius 16px, padding 24px.
- Result card (Next): bg `#f7f7f7`, radius 12px, padding 16px. Outline variant: border 1px
  `#dedede`, radius 12px, padding 0 16px.
- Floating tooltip card: white, radius 8px, padding 16px 24px, shadow `--shadow-2`.
- Choice card (get-started): white, border 1px `#888888`, radius 12px, padding 16px (32px ≥ sm).
- Hover: none observed on any card.

### Input (calculator)
- Wrapper: border 1px `#e2e8f0`, radius 6px, height 50px (input 48px; 44px below md).
- Input: bg `#ffffff`, text `#101010`, Inter 400 16/24 (14px below md), padding 8px 12px.
- Focus: wrapper border `#000000` plus a 1px ring (`box-shadow: 0 0 0 1px #000`); input's own
  outline removed.
- Select: same box, padding 12px 13px, chevron icon stroked in `oklch(55.1% .027 264.364)`.
- Range slider: track 6px high, radius 8px, filled part `#101010`, rest `#cccccc`.
- Search pill (FAQ): bg `#efefef`, border 1px `#d9d9d9`, radius 50px, padding
  17.6px 40px 17.6px 22.4px; input text Inter 500 26/36.4.
- Hover / active: not observed.

### Nav bar
- Height 72px desktop / 60px mobile; bg `rgba(255,255,255,0.9)` with `backdrop-filter: blur(20px)`;
  no border, no shadow. Container max 1202px, padding 16px.
- Links: Inter 400 14/20 (−0.42px on Next), text `#101010`, padding 8px 12px, radius 8px;
  hover bg `#dedede`; current page Inter 600.
- Log-in pill: white, radius 32px, padding 8px 12px 8px 8px, `--shadow-1`; hover shadow
  `0 6px 32px rgba(0,0,0,0.12)`, 150ms.
- Account button: 40px circle, bg `#efefef`; hover adds the 0.12 shadow.
- Mobile: hamburger + "Get started" pill; menu open/close via Webflow interactions (500ms bars,
  150ms `inCubic` rotate).

### Footer
- bg `#f7f7f7`; text `#29292b` / `#353535`; links `#0066cc`, 14px 400, no underline, underline
  on hover.

## 4. Motion notes

- **Button feedback:** colour-only. bg/border cross-fade 200ms `ease` (Webflow) or
  150–300ms `cubic-bezier(0.4,0,0.2,1)` (Next). No scale or lift on hover.
- **Segmented toggle:** white thumb slides under the label, `transform 350ms` (ease), label
  colour 350ms. Rate values switch with an opacity swap: out 250ms, in 250ms after a 250ms
  delay (Webflow interaction `rate-change-switch-out`).
- **Gradient text sweep:** FAQ categories move a pink→yellow gradient across the text by
  animating `background-position` over 275ms `ease-in-out`.
- **Scroll reveal:** Webflow `slideInBottom` on headings when 0–20% in view, with
  `slideOutBottom` / `fadeOut` when leaving. Duration and easing are Webflow preset values,
  not exposed in the page config — *not observed*. Next pages define `bottom-up`
  (0.6s ease-in-out, forwards; staggered variant +0.3s delay) and `article-fade-in`
  (0.6s ease). Keyframe bodies are in cross-origin CSS — *not observed*.
- **Accordion:** FAQ height 350ms `ease`; Next `accordion-down/up` 300ms `ease-out`.
- **Carousel:** Splide slides fade `opacity 600ms cubic-bezier(0.25,1,0.5,1)`.
- **Loaders:** `loading-fill 2s ease-in-out infinite`, `pulse 2s cubic-bezier(0.4,0,0.6,1)
  infinite`, `spin 1s linear infinite` (Tailwind defaults on the Next pages).
- **Ambient:** decorative rotations on the home page (`tron-rotate` 1000ms,
  `Apply_phone-bloom-rotate` 10000ms) start on page load.
- **Number flip:** the home rate sits in a `sp_flip-numbers-container` (a flip/counter widget
  from `cdn.unloan.com.au`). Its animation CSS is cross-origin — *not observed*.
- **Reduced motion:** no site-level `prefers-reduced-motion` rules (only the chat widget has
  them). We should add our own.

## 5. Overall feel

- **Light and monochrome.** White pages, `#f7f7f7` grey blocks, near-black `#101010` type and
  buttons. Colour arrives only as small accents (a lime `#f2ff00`, a pink→amber gradient) and
  link blue.
- **Flat, with soft lift where it matters.** Cards are flat fills or 1px hairlines; shadows are
  rare, very diffuse (40px blur, 7% black) and reserved for floating controls. The nav is
  frosted glass.
- **Rounded but not bubbly.** 16–24px on cards, 6–8px on inputs and chips, full 32px pills on
  every button.
- **Big type, airy layout.** Huge display headlines (80px desktop, 48px mobile) and 150px rate
  digits; generous 96px section padding; dense body copy at 14–16px with −3% tracking on
  headings.
- **Imagery:** cut-out lifestyle photos of people placed over giant bold words, plus simple
  line icons and a few 3D-ish illustrations on dark cards. (Described only; nothing copied.)

## How to capture again

1. Open https://www.unloan.com.au in desktop Chrome or Firefox (from Australia), then open
   DevTools → Console.
2. Paste the snippet below and press Enter. It returns (and copies to the clipboard) the
   most-used colors, backgrounds, borders, font stacks, radii, shadows, transitions and
   animations, sample button and heading styles, `@keyframes`, and any `:root` custom
   properties.
3. On the Next.js pages (`/home-loan`, `/calculators/*`, `/get-started`) `rootVars` comes back
   empty because the CSS is cross-origin; read
   `getComputedStyle(document.documentElement)` custom properties instead.

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
