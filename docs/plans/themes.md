# Kade colour themes: design and implementation plan

Status: plan only, no product code. Contrast numbers below were computed with a WCAG 2.x relative-luminance script (scratchpad `c.js`, `t.js`); every value in the tables passes the thresholds listed in section 4.

## 1. Token audit

Source of truth today: `src/lib/styles.css`. Light values in `:root`, dark values duplicated in `@media (prefers-color-scheme: dark) :root:not([data-theme="light"])` and `:root[data-theme="dark"]`. Counts are `var(--x)` references in `src/`.

| Token | Uses | Semantic role |
|---|---|---|
| `--snow` | 8 | App background, behind panes |
| `--paper` | 38 | Surface: panes, dialogs, inputs, cards |
| `--rail` | 3 | Sidebar / rail surface |
| `--granite` | 19 | Primary text ("ink") |
| `--ink2` | 58 | Secondary text, ghost button text, icons |
| `--lichen` | 70 | Muted text, `.lbl` labels, hints |
| `--faint` | 2 | Disabled text (`.btn:disabled`), input placeholder (ConnectDialog:258) |
| `--mist` | 44 | Default border / divider |
| `--mist2` | 49 | Hover background, disabled button fill, subtle fill |
| `--line-strong` | 4 | Emphasised divider |
| `--control` | 5 | Border of checkboxes, switches, radios (non-text UI edge) |
| `--zebra` | 1 | Alternate table row |
| `--glass` | 2 | Translucent overlay fill |
| `--pine` | 68 | Accent: primary button, links, focus ring, active nav, progress, drop target |
| `--pine-hover` | 2 | Primary button hover |
| `--pine-t` | 26 | Accent tint: selected row, active chip, drop-target fill |
| `--on-pine` | 2 | Text on accent |
| `--amber`, `-t`, `-ink`, `-line` | 9/5/3/1 | Warning (host-key change, unsaved) |
| `--danger`, `-hover`, `-t`, `-line` | 31/1/6/2 | Destructive, errors |
| `--on-danger` | 7 | Text on danger fill (toasts) |
| `--op-blue` | 1 | 1Password brand mark, not part of the theme |
| `--inverse`, `--on-inverse` | 2/5 | Dark chip: toasts, drag ghost |
| `--scrim` | 2 | Modal backdrop |
| `--shadow-lg/md/sm` | 3/5/7 | Elevation |
| focus ring | n/a | `:focus-visible { outline: 2px solid var(--pine) }` |
| selection | n/a | Not themed (browser default), xterm has its own |

Hardcoded colours that must become tokens:

| Where | Value | Becomes |
|---|---|---|
| `TerminalView.svelte` xterm `theme` (lines 56-78) | 22 hex | `--term-*` tokens, read at runtime (section 2) |
| `TerminalView.svelte` styles (143-195) | `#0f1513 #1a2420 #1c2723 #18221e #e8ece8 #5fc79e #6b7872 #233029 #c9d1cc #f08a78` | `--term-bg`, `--term-raise`, `--term-line`, `--term-chip`, `--term-15`, `--term-2`, `--term-8`, `--term-line2`, `--term-fg`, `--term-9` |
| `Sidebar.svelte:300` | `color: #fff` on workspace chip | `--on-ws` (white fails on amber, 3.6:1) |
| `WorkspaceDialog.svelte:190` | `<Check color="#fff">` | same `--on-ws` per swatch |
| `workspaces.ts` | six hex, one set for both schemes | per-scheme values (see 2.4); dark dots fail 3:1 today (slate 2.0, pine 2.6, plum 2.8) |
| `ConnectDialog.svelte:258` | placeholder uses `--faint` (2.2-2.7:1) | `--lichen`; placeholder text needs 4.5:1 |

Existing contrast failures in current Pine (these exist before any theme work):
- `--lichen` on paper 3.73:1, on snow 3.47:1 (used at 11px in `.lbl` and 70 places). Fails AA.
- `--control` on paper 1.84:1 light, 2.17:1 dark. Checkbox/switch edges fail the 3:1 non-text rule.
- Dark workspace dots (above).

Naming: tokens are pine-flavoured (`--pine`, `--on-pine`). With four palettes `--pine` means "accent". Do not rename in this change: other agents are editing components. Phase 1 introduces `--accent*` in palettes and keeps `--pine: var(--accent)` aliases; a mechanical rename sweep (`--pine` to `--accent`) is a separate follow-up commit.

## 2. Theme model

### 2.1 Two independent axes

- **Mode** (existing): `auto | light | dark`, stored `kade.theme`.
- **Palette** (new): `pine | haven | duin | schemer`, stored `kade.palette`, default `pine`.

Every palette defines a light and a dark token set. Any palette combines with any mode.

### 2.2 Storage: localStorage per machine

Recommendation: localStorage, same as mode and language (`kade.theme`, `kade.language`).
- It must be readable synchronously before first paint (`app.html` inline script). Synced `Settings` arrive via async Tauri `invoke`, so a synced palette would flash Pine on every launch.
- It is a per-display preference (a dark Haven laptop and a light Duin desktop is a legitimate setup), like the editor choice that is already "this computer only".
- Zero migration and no Rust or `kade.json` schema change, so no merge conflicts with the backend agents.
- Cost: not carried to a second machine. Acceptable; revisit only if users ask.

### 2.3 Application: `data-palette` + `data-scheme`

Today the dark block exists twice (media query plus `data-theme="dark"`), which would become 8 blocks. Instead JS always resolves the effective scheme and writes it:

```html
<html data-palette="haven" data-scheme="dark">
```

- `data-scheme` is `light|dark`, resolved from mode (`auto` uses `matchMedia("(prefers-color-scheme: dark)")`, with a `change` listener while mode is `auto`). The `@media` dark block disappears from CSS entirely.
- `data-theme` is kept only as the stored-mode concept in JS; CSS no longer reads it.
- `app.html` inline script (pre-paint) sets both attributes. Allowed palette ids are hardcoded there with a comment pointing at `theme.svelte.ts`; unknown id falls back to `pine`.
- CSS is one file `src/lib/themes.css` imported by `styles.css`:

```css
:root, [data-palette="pine"] { /* pine light + term tokens + shared status tokens */ }
[data-palette="pine"][data-scheme="dark"] { /* pine dark */ }
[data-palette="haven"] { ... }  [data-palette="haven"][data-scheme="dark"] { ... }
```

  Rules are element-level compound selectors (not `:root`-only) so the Settings preview cards can set `data-palette` and `data-scheme` on themselves and render any palette in either scheme without touching the root. `color-scheme` is set in each scheme block.
- Palette blocks define: surfaces, text, borders, accent family, `--inverse/--on-inverse`, `--term-*`. Shared in plain `:root` (not per palette): `--amber*`, `--danger*` (Duin overrides danger, see 3.3), `--op-blue`, `--scrim`, shadows, `--glass`, fonts.
- Light and dark shadow values stay as they are (neutral black), only `--shadow-*` in dark remain in the dark blocks of each palette via a shared selector `[data-scheme="dark"]` placed before palette blocks (palette blocks never set shadows).

### 2.4 Derived colours

**Workspace colours** (`workspaces.ts`). Workspace colours are identity, so the six hues stay the same in every palette; only the scheme changes them. Replace the TS constants with CSS custom properties per scheme and let `WORKSPACE_COLORS` return `var(--ws-pine)` etc.:

| key | light dot | dark dot | on-ws light | on-ws dark |
|---|---|---|---|---|
| pine | `#1d6b57` | `#3a9a80` | `#ffffff` | `#14110a` |
| blue (Fjord) | `#2f6fb0` | `#5b93d0` | `#ffffff` | `#14110a` |
| amber | `#b7791f` | `#c98a2e` | `#14110a` | `#14110a` |
| plum | `#8a4f8f` | `#b072b5` | `#ffffff` | `#14110a` |
| coral | `#c2553f` | `#d9715a` | `#ffffff` | `#14110a` |
| slate | `#4a524e` | `#87918b` | `#ffffff` | `#14110a` |

Verified: dots on paper 3.6-8.1:1 (light) and 4.6-5.8:1 (dark) in all four palettes; white or dark-ink glyph on the dot at least 4.5:1 except coral-light 4.5 (passes) and amber needs dark ink. `tint` and `ink` keep the existing `color-mix` against `--paper` and `--granite` (ink on tint 4.4-8.0:1 in all 8 variants, minimum is amber-light in Duin, 4.4; bump the ink mix from 72% to 78% to clear 4.5, re-verify in phase 4).

Collision note: each accent sits near one workspace colour by design (Pine/pine, Haven/blue dE 11-21, Duin/coral dE 9-21, Schemer/plum dE 15-29), so the six workspace colours are never confusable with each other (min pairwise dE 25.5, slate/pine) and the accent never lands on an unrelated hue. A workspace that shares the accent hue is distinguished by its initial/label and position, not hue alone; the chip never relies on colour only. Check this by eye in phase 4.

**Terminal.** xterm theme is built from CSS custom properties, one source of truth:
- Tokens `--term-bg --term-fg --term-cursor --term-sel --term-0 ... --term-15` plus chrome `--term-raise --term-line --term-line2`, defined in the palette base block (scheme independent).
- Decision: the terminal stays dark in both schemes, as today (dark terminals are the convention, and a light terminal would need four more AA-checked ANSI sets). The palette still drives it: bg, chrome and ANSI hues are tinted per palette. A "light terminal in light mode" option is a clean follow-up (add `[data-scheme="light"]` term blocks).
- New `src/lib/termTheme.ts`: `termTheme(): ITheme` reads `getComputedStyle(document.documentElement)` and maps tokens to xterm keys.
- `TerminalView.svelte`: `theme: termTheme()` on create, and `$effect(() => { theme.palette; term.options.theme = termTheme(); })` so open terminals recolour live. `applyTheme` mutates the DOM before state-driven effects run, so the read is fresh.

## 3. The four themes

Names are short Dutch/harbour words in the spirit of "Kade". Proper nouns, not translated.

| Id | Name | Character | Why |
|---|---|---|---|
| `pine` | Pine | Alpine hut green, today's look | Default, brand colour (site/index.html uses `#1D6B57`) |
| `haven` | Haven | Cool steel blue and slate, a harbour in grey morning light | The obvious second choice for a network tool; neutral, calm, most "tool-like" |
| `duin` | Duin | Warm sand with clay accent, dune at low sun | The warm counterpart; terracotta accent, brown-tinted dark mode instead of grey |
| `schemer` | Schemer | Muted dusk violet, harbour at twilight | A low-saturation purple for people who live in dark mode; avoids neon "hacker purple" |

Why not high-contrast as the fourth: all 8 variants already pass AA, and a true high-contrast palette (7:1 body, 2px borders) deserves its own `forced-colors`/`prefers-contrast: more` pass. Listed as a follow-up.

Duin specifics: terracotta sits next to the shared danger red, so Duin overrides danger to a crimson (`#b3263e` light, `#f0788c` dark, hue about 350 vs accent about 14) and uses `--accent` only for primary actions, never for errors. Warning amber is still distinguishable by its tinted background and icon.

Pine changes marked with a dagger are the only deviations from today's values; they fix the existing AA failures from section 1 and are small (muted text slightly darker, control edges visible). Every other Pine value is untouched.

### 3.1 Pine

| Token | Light | Dark |
|---|---|---|
| snow | `#f6f7f4` | `#141816` |
| paper | `#ffffff` | `#1b201e` |
| rail | `#eef1ec` | `#171b19` |
| granite | `#1e2321` | `#e6eae7` |
| ink2 | `#4a524e` | `#b3bbb6` |
| lichen | `#66716b` † (was `#7c8780`) | `#87918b` |
| faint | `#98a19b` † (was `#a9b1ac`) | `#6c7670` † (was `#5f6964`) |
| mist | `#e4e8e3` | `#2b322f` |
| mist2 | `#eef1ed` | `#242a27` |
| line-strong | `#cfd6d1` | `#3a423e` |
| control | `#889490` † (was `#b9c1bc`) | `#6a746f` † (was `#4d5651`) |
| zebra | `#fafbf9` | `#1e2321` |
| accent | `#1d6b57` | `#4fb393` |
| accent-hover | `#185a49` | `#63c4a4` |
| accent-t | `#e3efea` | `#1e3a31` |
| on-accent | `#ffffff` | `#0b1d17` |
| inverse / on-inverse | `#1e2321` / `#ffffff` | `#e6eae7` / `#141816` |

Terminal: bg `#0f1513`, fg `#d9dfdb`, cursor `#5fc79e`, selection `#2a3d35`, raise `#1a2420`, line `#1c2723`, line2 `#233029`.
ANSI 0-15: `#18221e #e06c5a #5fc79e #e0a84a #8fb8e8 #c79bd8 #6fc3c0 #d9dfdb` / `#7d8a84 † #f08a78 #7fdcb5 #f0c070 #aacdf2 #dab3e8 #8fd8d5 #f4f6f4`.

### 3.2 Haven

| Token | Light | Dark |
|---|---|---|
| snow | `#f3f6f9` | `#10161d` |
| paper | `#ffffff` | `#18202a` |
| rail | `#e9eef3` | `#131a22` |
| granite | `#17212b` | `#e4eaf1` |
| ink2 | `#435262` | `#b0bccb` |
| lichen | `#5f6f80` | `#8a98aa` |
| faint | `#9aa8b6` | `#667385` |
| mist | `#dde5ec` | `#26313e` |
| mist2 | `#e9eef3` | `#212b37` |
| line-strong | `#c5d0da` | `#34414f` |
| control | `#8493a3` | `#6a7889` |
| zebra | `#f8fafc` | `#1b2430` |
| accent | `#2559a0` | `#6aa6ea` |
| accent-hover | `#1d4a87` | `#82b5ee` |
| accent-t | `#e1ebf7` | `#1b3250` |
| on-accent | `#ffffff` | `#08182b` |
| inverse / on-inverse | `#17212b` / `#ffffff` | `#e4eaf1` / `#10161d` |

Terminal: bg `#0d141b`, fg `#d6dee7`, cursor `#6aa6ea`, selection `#233a55`, raise `#16202b`, line `#19232e`, line2 `#223042`.
ANSI: `#17212b #e4786a #68c49a #e0b050 #78aef0 #c3a0e6 #5cc6d0 #d6dee7` / `#7a8aa0 #f2907f #86d9b0 #efc472 #9cc6f6 #d6b8f0 #86dae2 #f3f7fb`.

### 3.3 Duin

| Token | Light | Dark |
|---|---|---|
| snow | `#f7f3ec` | `#1a1613` |
| paper | `#fffdf9` | `#231e1a` |
| rail | `#efe9de` | `#1e1a16` |
| granite | `#2a231c` | `#efe8de` |
| ink2 | `#5a4e42` | `#c0b5a6` |
| lichen | `#75675a` | `#9d9181` |
| faint | `#a89c8c` | `#6f6558` |
| mist | `#e8e0d3` | `#352e27` |
| mist2 | `#f0eadf` | `#2b2520` |
| line-strong | `#d6ccbb` | `#453d34` |
| control | `#948675` | `#796d5f` |
| zebra | `#fbf8f2` | `#27221d` |
| accent | `#a8472a` | `#e58d66` |
| accent-hover | `#8f3c23` | `#ee9f7b` |
| accent-t | `#f6e4dc` | `#43271c` |
| on-accent | `#ffffff` | `#2a1209` |
| danger override | `#b3263e` / hover `#96203a` / t `#f8e3e6` / line `#eec3ca` / on `#ffffff` | `#f0788c` / hover `#f58ea0` / t `#42222a` / line `#62303a` / on `#2a0a12` |
| inverse / on-inverse | `#2a231c` / `#ffffff` | `#efe8de` / `#1a1613` |

Terminal: bg `#17120f`, fg `#e4dacd`, cursor `#e58d66`, selection `#4a3226`, raise `#211a15`, line `#231c17`, line2 `#30261e`.
ANSI: `#241d18 #ee7a70 #9fcf7a #e8b24f #7fb0e0 #d49ac4 #6cc3b4 #e4dacd` / `#948777 #f89a90 #b8e094 #f2c872 #a0c6ec #e4b4d6 #8fd6c8 #f8f2e9`.

### 3.4 Schemer

| Token | Light | Dark |
|---|---|---|
| snow | `#f5f3f7` | `#16131c` |
| paper | `#ffffff` | `#1e1a26` |
| rail | `#ece9f1` | `#191620` |
| granite | `#211d2b` | `#e9e6ef` |
| ink2 | `#4e4860` | `#b9b3c8` |
| lichen | `#6b647d` | `#968fa8` |
| faint | `#a39db3` | `#6a6379` |
| mist | `#e4e0ea` | `#2d2839` |
| mist2 | `#ece9f1` | `#262230` |
| line-strong | `#d0cada` | `#3d374c` |
| control | `#8d869f` | `#7a7390` |
| zebra | `#faf9fc` | `#221e2b` |
| accent | `#6a4a99` | `#b49be0` |
| accent-hover | `#593d82` | `#c5b0ea` |
| accent-t | `#ebe5f4` | `#2f2745` |
| on-accent | `#ffffff` | `#1a1128` |
| inverse / on-inverse | `#211d2b` / `#ffffff` | `#e9e6ef` / `#16131c` |

Terminal: bg `#120f19`, fg `#dcd7e6`, cursor `#b49be0`, selection `#33294d`, raise `#1b1726`, line `#1d1929`, line2 `#2a2339`.
ANSI: `#1e1a2a #ec7a8e #7fd0a4 #e6b86a #86a8ee #c9a0f0 #6cc7d4 #dcd7e6` / `#8379a0 #f493a4 #9ce0ba #f0cb8a #a8c0f6 #dcb8f6 #8fdae4 #f6f3fb`.

## 4. Contrast check (computed)

Thresholds: 4.5 body text, 3.0 large text and UI edges and focus ring. `faint` is disabled/decorative only (exempt), but kept at least 2.2 on paper so it reads as "lighter than lichen" and the disabled fill (`mist2`) still differs from enabled buttons. Amber/danger shared tokens are included.

| Pair | Min | Pine L | Pine D | Haven L | Haven D | Duin L | Duin D | Schemer L | Schemer D |
|---|---|---|---|---|---|---|---|---|---|
| ink / paper | 4.5 | 15.9 | 13.6 | 16.3 | 13.6 | 15.2 | 13.6 | 16.5 | 13.8 |
| ink / snow (app bg) | 4.5 | 14.8 | 14.8 | 15.0 | 15.0 | 14.0 | 14.8 | 14.9 | 14.9 |
| ink2 / paper | 4.5 | 8.1 | 8.4 | 8.0 | 8.5 | 7.9 | 8.2 | 8.7 | 8.4 |
| ink2 / mist2 (hover) | 4.5 | 7.1 | 7.5 | 6.9 | 7.4 | 6.7 | 7.5 | 7.2 | 7.6 |
| lichen / paper | 4.5 | 5.1 | 5.1 | 5.2 | 5.6 | 5.4 | 5.3 | 5.6 | 5.5 |
| lichen / snow | 4.5 | 4.7 | 5.5 | 4.8 | 6.2 | 4.9 | 5.8 | 5.1 | 5.9 |
| accent / paper (links) | 4.5 | 6.4 | 6.4 | 7.0 | 6.5 | 5.7 | 6.6 | 6.9 | 7.1 |
| accent / accent-t (selected) | 4.5 | 5.4 | 4.8 | 5.8 | 5.1 | 4.7 | 5.4 | 5.6 | 5.8 |
| on-accent / accent (button) | 4.5 | 6.4 | 6.8 | 7.0 | 7.0 | 5.8 | 7.0 | 6.9 | 7.5 |
| on-accent / hover | 4.5 | 8.1 | 8.3 | 8.8 | 8.3 | 7.4 | 8.3 | 8.7 | 9.3 |
| danger / paper | 4.5 | 5.6 | 5.9 | 5.6 | 5.9 | 6.3 | 6.1 | 5.6 | 6.1 |
| danger / danger-t | 4.5 | 4.6 | 5.1 | 4.6 | 5.1 | 5.2 | 5.2 | 4.6 | 5.1 |
| on-danger / danger | 4.5 | 5.6 | 6.7 | 5.6 | 6.7 | 6.4 | 6.8 | 5.6 | 6.7 |
| amber-ink / amber-t | 4.5 | 6.0 | 8.0 | 6.0 | 8.0 | 6.0 | 8.0 | 6.0 | 8.0 |
| on-inverse / inverse (toast) | 4.5 | 15.9 | 14.8 | 16.3 | 15.0 | 15.5 | 14.8 | 16.5 | 14.9 |
| control / paper (checkbox, switch edge) | 3.0 | 3.1 | 3.4 | 3.1 | 3.6 | 3.5 | 3.3 | 3.5 | 3.8 |
| accent / paper (focus ring) | 3.0 | 6.4 | 6.4 | 7.0 | 6.5 | 5.7 | 6.6 | 6.9 | 7.1 |
| accent / mist2 (ring on hovered row) | 3.0 | 5.6 | 5.7 | 6.0 | 5.6 | 4.9 | 6.0 | 5.7 | 6.4 |
| faint / paper (decorative) | 2.2 | 2.7 | 3.5 | 2.4 | 3.4 | 2.7 | 2.9 | 2.6 | 3.0 |
| faint / mist2 (disabled label) | 2.0 | 2.3 | 3.1 | 2.1 | 3.0 | 2.2 | 2.6 | 2.2 | 2.7 |

Terminal (text on `--term-bg`, all 15 non-black ANSI colours must be at least 4.5):

| Palette | fg | cursor | fg on selection | brightBlack | lowest ANSI colour |
|---|---|---|---|---|---|
| Pine | 13.7 | 8.9 | 8.5 | 5.1 † (4.0 before) | 5.1 |
| Haven | 13.7 | 7.3 | 8.6 | 5.3 | 5.3 |
| Duin | 13.5 | 7.4 | 8.6 | 5.3 | 5.3 |
| Schemer | 13.5 | 7.9 | 9.5 | 4.7 | 4.7 |

ANSI "black" is a chip/background colour (1.1:1 against bg by design, like every terminal theme); programs that print black text onto coloured backgrounds are unaffected.

Disabled buttons: `.btn:disabled` keeps its current recipe (flat `mist2` fill, `mist` border, `faint` text, `cursor: not-allowed`, no hover). In every variant the fill differs from the enabled primary button by far more than 3:1 in luminance and the label is visibly lighter than `lichen`.

Two rules for implementers: placeholders use `--lichen`, never `--faint`; any new text on the accent uses `--on-accent`.

## 5. Settings UI

Location: `SettingsPage.svelte`, section General (the page has no "Appearance" heading today; the Theme row at line 256 is the right spot). Rename the existing row to "Mode" and add a new row "Colour theme" directly under it.

```
Mode            [ Automatic | Light | Dark ]
Colour theme    Four palettes, each in light and dark.
  ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐
  │ preview│ │ preview│ │ preview│ │ preview│
  │ Pine ● │ │ Haven  │ │ Duin   │ │ Schemer│
  └────────┘ └────────┘ └────────┘ └────────┘
```

- Four cards in a grid (`repeat(auto-fit, minmax(132px, 1fr))`), below the row label rather than right-aligned (cards are too wide for the segmented slot).
- Each card is a `<label>` wrapping a visually hidden `<input type="radio" name="palette">`, same pattern as the existing `segmented` snippet, inside `role="radiogroup" aria-label={t("Colour theme")}`. Native radios give roving arrow keys, Space to select, and Tab entering the group once, for free; no custom key handling. Use the existing `radioGroup.ts` helper only if it is already the project pattern for custom groups.
- The card root sets `data-palette={id} data-scheme={theme.scheme}`, so the preview is painted by the real tokens in the user's current scheme. Mini preview (about 120x68, `aria-hidden`): `--snow` ground, a `--rail` strip on the left with three `--mist` lines, a `--paper` pane with two `--ink2` text bars, one `--accent` pill, one `--accent-t` selected row. Below it the name in `--granite`, and a check icon when selected.
- Selected: 2px `--accent` border (the card's own palette accent would be confusing; use the app's current `--accent` for the selection chrome by drawing the border on an outer wrapper that is not inside the `data-palette` element) plus the visually hidden radio `:checked`. Focus: `:focus-within` shows the global 2px focus ring on the card. Not colour-only: selected also shows a check icon.
- Hover: `--mist2` background on the wrapper. Selecting applies instantly (no confirm), consistent with Mode.
- The name label is the radio's accessible name; the preview is `aria-hidden`.

Strings (en / nl, via `t()` and `src/lib/locales/nl.ts`):

| English key | Dutch |
|---|---|
| Mode (replaces "Theme" on the existing row; update its `aria-label` too) | Weergave |
| Colour theme | Kleurthema |
| Each theme has a light and a dark version. | Elk thema heeft een lichte en een donkere versie. |
| Automatic follows your system. (existing) | unchanged |

Palette names (Pine, Haven, Duin, Schemer) are not translated. If "Theme" is still used elsewhere in nl.ts (line 78 `"Theme": "Thema"`), keep it; only the row label changes.

## 6. Implementation phases

Touches are listed so they can be sequenced around the other agents. Phases 1-2 have no visible change for Pine users except the † fixes.

**Phase 1: tokens and attributes** (no new UI)
- New `src/lib/themes.css` with all palette blocks and shared status tokens; `styles.css` removes its two dark blocks and imports it, keeps `--sans/--mono`, adds `--pine*: var(--accent*)` aliases.
- `src/lib/theme.svelte.ts`: add `palette` and resolved `scheme` state, `PALETTES` list, `matchMedia` listener while mode is `auto`, `applyTheme` writes `data-palette` and `data-scheme`; keep exported `ThemeChoice`.
- `src/app.html`: inline script sets `data-palette` and `data-scheme` before paint.
- Remove the `@media (prefers-color-scheme)` CSS entirely; grep `src/` for `data-theme` selectors in components and fix any.

**Phase 2: remove hardcoded colours**
- `workspaces.ts` to CSS vars (`--ws-*`, `--on-ws-*`) defined per scheme in `themes.css`; `Sidebar.svelte:300` and `WorkspaceDialog.svelte:190` use `--on-ws`; ink mix 72% to 78%.
- `ConnectDialog.svelte:258` placeholder to `--lichen`.
- `TerminalView.svelte`: scoped styles to `--term-*`; new `src/lib/termTheme.ts`; `$effect` for live recolour.

**Phase 3: picker**
- `SettingsPage.svelte`: new row and card grid, styles scoped in the component; `nl.ts` strings.

**Phase 4: verify, then optional rename sweep**
- Run the test plan below; after the parallel work lands, rename `--pine*` to `--accent*` repo-wide in one mechanical commit and drop the aliases.

Risks:
- Cards that render a non-active palette depend on element-level `data-palette` selectors; do not scope palette blocks to `:root`.
- `getComputedStyle` must run after the attribute change; keep `applyTheme` synchronous.
- `site/index.html` is a separate dark brand page and is not themed.

## 7. Test plan

1. Matrix: for each of Pine, Haven, Duin, Schemer x Light, Dark (8 variants), open file view, connect dialog, settings, tunnels, backups dialog, quick switcher, toasts (success and error), drag ghost; check nothing keeps the old green (search for pine-coloured pixels in Haven/Duin/Schemer: accent buttons, focus ring, selected rows, progress, drop target, spinner).
2. Mode independence: with palette Haven, switch Automatic/Light/Dark; with Automatic, flip the OS appearance while the app is open and confirm the scheme and an open terminal follow without reload.
3. First paint: set each palette, reload; no flash of Pine (throttle CPU in devtools to check). Corrupt `kade.palette` to `nonsense` and confirm Pine loads.
4. Terminal: in each palette run `ls --color`, `htop` or a 16-colour script (`for i in {0..15}; do printf "\e[38;5;${i}m%3d " $i; done`); confirm all colours readable, cursor visible, selection legible, and an already-open terminal recolours the moment the palette changes. Header chip, restart button, error banner use the tokens.
5. Workspace accents: create six workspaces (one per colour); in every variant confirm sidebar chips, tab stripes, and the colour pickers are distinct, white or dark glyph readable (amber), and the workspace that matches the accent hue (pine/blue/coral/plum) is still told apart by initial and position.
6. Focus ring: Tab through settings, a dialog, and the file list in all 8 variants; ring visible on `paper`, `mist2` hover and `accent-t` selected rows.
7. Disabled and states: disabled primary and secondary buttons look disabled (not just lighter); danger button, warning banners and error text in Duin remain distinct from the terracotta accent.
8. Picker: arrow keys move and select within the radiogroup, Tab enters and leaves once, selected card shows a check and ring, screen reader reads "Colour theme, radio group, Haven, 2 of 4"; works at the Settings page's narrowest width (cards wrap).
9. Automated: re-run the contrast script against `themes.css` values (parse the file) as a unit test so future token edits cannot regress AA; assert every palette defines the same set of custom properties.
