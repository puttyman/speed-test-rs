# Design tokens

The project's visual language: monochrome and light-first, with generous whitespace, pill
buttons and flat cards. Color is reserved for data and status. The tokens are implemented in
[`style/_tokens.scss`](../../style/_tokens.scss); components use the custom properties only,
never raw values. Rationale and motion rules are in [SPEC §7](../SPEC.md).

## 1. Color

| Role | Light | Dark |
|---|---|---|
| Brand (buttons, headings) | `#101010` on `#ffffff` | `#ffffff` on `#171d26` |
| Brand hover / active | `#353535` / `#676767` | `#dedede` / `#a2a2a2` |
| Soft fill (tracks, chips) | `#efefef` | `#2a313c` |
| Surface (cards) | `#f7f7f7` | `#202733` |
| Hairlines | `#dedede`, strong `#cccccc` | `#323a46`, strong `#404042` |
| Text: heading / body / muted | `#101010` / `#29292b` / `#6d6b6d` | `#ffffff` / `#e6e7ea` / `#a3a8b1` |
| Link | `#0066cc` | `#6aa8ff` |
| Accent (brand mark only) | gradient `#ff7bad → #ffba2f`; lime `#f2ff00`, never as text on light | same |

**Data series** (validated for color-vision deficiency and contrast against the surface, all
pairs, both themes): download `#0066cc` / `#3987e5`, upload `#d6457f` / `#d55181`, latency
`#bf7400` / `#c98500`.

**Per-device colors** (group tests and compare), fixed slot order blue, orange, aqua, yellow,
magenta, green, violet, red: `--device-1` … `--device-8`. Above three series, charts add direct
labels, so identity never depends on color alone.

**Status badges** use a soft background with strong text, e.g. great `#eff3f0` / `#005011`,
average `#fff4e7` / `#7d4900`, poor `#fbeff0` / `#d70015`.

## 2. Type

| Use | Font | Notes |
|---|---|---|
| Display headings | IBM Plex Mono 500 | Tracking −3% |
| UI and body | Inter (variable 100–900) | |
| Numbers | Inter 700 | `font-variant-numeric: tabular-nums` so values don't shift |

Both fonts are OFL and self-hosted in `public/fonts/`. The scale runs from `--text-xs`
(0.75rem) to a fluid `--text-hero` (`clamp(3.5rem, 2rem + 7vw, 9.375rem)`).

## 3. Shape, space and depth

- Radii: inputs and chips 8px, cards 16px (large cards 24px), every button a 32px pill.
- Space: 4px base (`--space-1` … `--space-24`); content width 1170px.
- Depth: flat fills. A diffuse `0 3px 40px rgba(0,0,0,.07)` only on floating controls; the nav
  is frosted glass (`blur(20px)`).

## 4. Components

- **Primary button**: `--color-brand` fill, white text, 32px pill, Inter 600; hover
  `--color-brand-hover`, active `--color-brand-active`; color-only transition at 200ms.
- **Secondary button**: transparent with a 1.5–2px brand border; hover fills `--color-brand-soft`.
- **Segmented control** (units, transport, theme): a `#efefef` track (radius 8px, padding 2px);
  options Inter 600 14/20; a white thumb (radius 6px, shadow `0 1px 2px rgba(0,0,0,.05)`) slides
  with `transform 350ms`, and labels change color over the same 350ms. Arrow keys move the
  selection.
- **Card**: `--color-surface`, radius 16px, padding 24px (16px sides on phones), no border.
- **Focus**: a visible 2px `--color-focus` ring on every interactive element.

## 5. Motion

`--ease-standard` `cubic-bezier(.4,0,.2,1)` for controls, `--ease-soft` `cubic-bezier(.25,1,.5,1)`
for reveals. Durations: 150ms (fast), 200ms (buttons), 350ms (segmented control), 600ms
(reveals). With reduced motion every duration is 0.
