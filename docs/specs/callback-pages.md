# Branded OAuth callback pages

## Goal

Replace the four plain OAuth login callback responses in `src/runtime.rs` with a
single branded, self-contained HTML page on the project design system, themed light
and dark, with a `Developed by pwguler` credit. One shared template serves both the
Anthropic and Codex flows (they share `handle_callback_stream`).

## Outcomes

Four callback outcomes, each a full HTML page (`Content-Type: text/html`):

| Outcome | Status | Glyph | Heading | Subtext |
|---|---|---|---|---|
| Success | 200 | green check | Login successful | You can close this tab and return to the terminal. |
| Provider error | 400 | red ✕ | Login failed | The provider returned an error: `<detail>`. Try again from the terminal. |
| Missing code / state | 400 | red ! | Login failed | The callback was missing its code or state. Try again from the terminal. |
| Wrong path | 404 | red ? | Not found | This isn't the pengepul login callback. |

Every page shows the `pengepul` wordmark, the glyph, heading, subtext, and a footer
`Developed by pwguler` linking `https://github.com/pwguler/pengepul`.

## Design

Follows the design system in `docs/DESIGN.md` (tokens in `docs/tokens.json` and
`docs/theme.css`, derived from the fontpair Space Grotesk & DM Sans kit). Clean, cool,
product-native. No 3D, no motion, no script.

- Palette is exactly the system's six tokens: background `#FFFFFF`, text `#002303`,
  primary `#10B981`, accent `#005526`, surface `#F1F3F1`, border `#DBE0DC`. No other
  color is introduced; there are no semantic status colors. Success and failure are told
  by copy, icon, and the eyebrow, not by hue. Primary is a fill only and never carries
  white text (2.54:1); when text sits on it, it is `text` (6.67:1).
- Card is `card-surface`: surface fill, text ink, `rounded.lg` 20px, 32px padding, a 1px
  `border` hairline, no shadow (depth comes from surface and hairline, per the system).
- Eyebrow above the heading is `caption` uppercase (DM Sans 500). On success it is a
  `badge-signal` reading `Signed in` (primary fill, text ink, `rounded.sm`): emerald marks
  the one thing that matters, and the win is it. On every failure it is plain caption
  text in ink reading `Error`; emerald is absent, nothing to signal.
- Icons are Feather, 2px stroke, round caps, stroked in `currentColor` (text), never
  filled: `check-circle` for success, `x-circle` for a provider error, `alert-circle`
  for missing params, `help-circle` for a wrong path.
- Footer link is the system's `link`: `accent` ink, underlined.
- Dark theme is `card-inverse` onto the page: text `#002303` ground and card, background
  white ink, a 1px accent rule as the card edge; the footer link inherits white there
  (accent on ink fails contrast; primary as link text is forbidden). Theme-aware via
  `prefers-color-scheme` and `data-theme`.
- Type, from the system's scale: wordmark in Space Grotesk 600 at 20px; heading at `h2`
  (Space Grotesk 600, 38px / 1.15 / -0.02em, stepping to `h3` metrics under 480px);
  subtext at `small` (DM Sans 400, 16px / 1.55); eyebrow and footer at `caption`
  (DM Sans 500, 13px / 1.5 / 0.04em) with `font-optical-sizing: auto`. Spacing uses the
  system's `md` 16 / `lg` 24 / `xl` 40 steps.
- Fonts are real and self-contained: Space Grotesk 600 subset to the heading and
  wordmark glyphs (2KB) and the DM Sans latin variable face (`wght` 100–1000, `opsz`
  9–40, 37KB; one file serves both the 400 body and 500 caption weights) vendored as
  woff2 under `assets/fonts/`, compiled into the binary with `include_bytes!`, and
  emitted as `data:font/woff2;base64` `@font-face` sources. Both are SIL OFL 1.1, and a
  subset is a Modified Version, so the `@font-face` families are aliased
  `pengepul-display` and `pengepul-body`; the OFL texts are vendored beside the fonts.

Reference mockup (throwaway, not shipped): the artifact published in this session.

## Non-goals

- No external assets of any kind on the served page: no CDN `<script>`, no external
  stylesheet, no web font, no remote image. The page is served once over a one-shot
  TCP socket and must render fully offline. The only `https://` in the markup is the
  footer credit anchor's `href` (a link the user clicks, not a fetched resource).
- No script of any kind. An inline-WebGL background was built and then dropped when the
  design system arrived; the page is static HTML and CSS.
- No change to OAuth logic, status codes, redirect URIs, ports, callback paths, the
  `CallbackResult` return, or the `bail!` error paths. Only the response bodies and
  their content type change.
- No `window.close()` auto-close (the tab was opened by `xdg-open`/`open`, not script;
  it would not work).

## Acceptance criteria

1. `handle_callback_stream`'s four `write_http_response` sites all send
   `Content-Type: text/html` with the branded page for their outcome.
   - Verify: `grep -n 'text/plain' src/runtime.rs` returns nothing inside
     `handle_callback_stream` (the 404/400 sites no longer use plain text).
2. Page rendering is a pure function (e.g. `callback_page_html(outcome) -> String`),
   unit-tested without a socket.
3. The provider error detail is HTML-escaped before embedding. A detail of
   `"><script>alert(1)</script>` must not appear unescaped in the output.
   - Verify: unit test asserts the rendered error page does not contain
     `<script>alert(1)` and does contain the escaped form `&lt;script&gt;`.
4. Success page contains `Login successful`; each failure page contains `Login failed`
   or `Not found` per the table; every page contains `pengepul` and the credit footer.
   The footer links the repo from the word `pwguler`, so its exact markup is
   `Developed by <a href="https://github.com/pwguler/pengepul">pwguler</a>` (the plain
   text `Developed by pwguler` is split by the anchor and is not a contiguous substring).
   - Verify: unit tests assert the heading per outcome and the exact footer markup.
5. Page is self-contained and static: rendered HTML contains no `<script` at all, no
   `<canvas`, no `<link `, no `src="http`, and no `@import`. (The footer
   `href="https://github…"` is the only external URL.)
   - Verify: unit test asserts absence of those substrings.
5a. Brand fonts are embedded, not fetched: the page contains two `@font-face` rules whose
   `src` is a `data:font/woff2;base64,` URI, aliased `pengepul-display` (Space Grotesk
   600) and `pengepul-body` (DM Sans variable, `font-weight:100 1000`); the heading and
   wordmark are set in `pengepul-display`, body copy in `pengepul-body`. No
   `fonts.googleapis.com` or `fonts.gstatic.com` reference anywhere.
   - Verify: unit test asserts the two aliases, the `data:font/woff2;base64,` prefix,
     and the absence of any Google Fonts host. The woff2 files and their OFL texts exist
     under `assets/fonts/` and are tracked by git.
5b. Palette is exactly the system's: every 6-digit hex color in the rendered page is one
   of `#FFFFFF`, `#002303`, `#10B981`, `#005526`, `#F1F3F1`, `#DBE0DC`, and all six
   appear. No other color exists on the page (no status reds, nothing from the previous
   cream or indigo designs). White text is never placed on primary: the badge's text
   token is `#002303`.
   - Verify: unit test scans the page for every `#xxxxxx` and asserts the set equals the
     six tokens (case-insensitive), and asserts the badge rule pairs `#10b981` with
     `#002303`.
5c. Icons are Feather outlines: each glyph SVG is `fill="none"`, `stroke="currentColor"`,
   `stroke-width="2"`, round caps and joins; success renders `check-circle`, failures
   render `x-circle` / `alert-circle` / `help-circle`. The success page carries the
   `Signed in` `badge-signal`; failure pages carry a plain `Error` eyebrow and no badge.
   - Verify: unit test asserts the stroke attributes on every page and the eyebrow/badge
     per outcome.
6. Existing behavior preserved: success still returns `CallbackResult { code, state }`;
   error/missing/wrong-path still `bail!` with their current messages and status codes
   (200 / 400 / 400 / 404).
   - Verify: existing tests still pass; the status codes in the `write_http_response`
     calls are unchanged.
7. Theme-aware: the CSS defines light on bare `:root`, redefines tokens under
   `@media (prefers-color-scheme: dark)` (guarded `:root:not([data-theme="light"])`)
   and `:root[data-theme="dark"]`. The dark theme uses the system's text token `#002303`
   as ground (`card-inverse`). There is no motion to guard: the page is static.

## Verification commands

```sh
cargo test --locked
cargo fmt --check
cargo clippy --all-targets --all-features --locked -- -D warnings
grep -n 'text/plain' src/runtime.rs   # none within handle_callback_stream
```

Manual (optional, not gating): `pengepul login`, complete in a browser, eyeball the
success page; force a failure (deny consent) and eyeball the error page; toggle OS
dark mode and reload.
