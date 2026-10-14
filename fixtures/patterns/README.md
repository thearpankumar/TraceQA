# Patterns

Pages built like real sites, one common pattern each. `tests/test_patterns.py`
runs the whole engine on every page in Chromium, Firefox and WebKit and checks
three things:

1. **The components.** Each page lists, in
   `<script type="application/json" id="expected-components">`, every component
   the engine must produce: role, name, number of instances, and whether the name
   is a template (`"template": true`, with the name `*`). Nothing may be missing
   and nothing may be added.
2. **The elements.** Elements carry `data-expect`, in the format of
   [the collector cases](../collector-cases/README.md), read here from
   `components.json`. Two more keys are available: `instances` (how many
   instances the element's component has) and `template` (`true` or `false`).
   `source` is not, because `components.json` does not record it.
3. **The ids.** Every browser must give every component the same id.

```sh
uv run pytest tests/test_patterns.py
TRACEQA_TEST_SAFARI=1 uv run pytest tests/test_patterns.py   # also real Safari, by hand
```

## Pages

| Page | Pattern | What it checks |
|---|---|---|
| `mega-menu.html` | Navigation with buttons that open hidden panels of links | Closed panels' links are listed as hidden; menu buttons report `expanded=false`; links in navigation are never merged |
| `carousel.html` | Slides with previous and next buttons and dots | Hidden slides' links are hidden; the same "Shop now" on every slide is one component; dots are separate |
| `cookie-banner.html` | A fixed banner at the bottom | A button under the banner is covered; the banner's own buttons are not |
| `login-form.html` | Email, password, show-password, remember me, sign-in buttons for other services | Names from labels, `required`, `pressed`, `checked`; three sign-in buttons stay three |
| `product-grid.html` | Filters, sort, and cards with a title, a price and two buttons | Titles are one templated component; each button is one component with eight instances |
| `tabs.html` | A tab list with hidden panels | `selected`; controls in hidden panels are hidden |
| `accordion.html` | `<details>` and a button that opens a region | `expanded`; controls in closed sections are hidden |
| `modal.html` | A dialog over a dimmed page | The page behind is covered; the dialog is not |
| `data-table.html` | Rows with a select box, Edit and Delete, sortable headers, paging | Each row's select box is one templated component across the rows |

## Adding a page

Copy a pattern from a real site, as plain HTML with no build step. State the
components it must produce, and add `data-expect` to the elements that show
what the pattern is about. A page whose expectations the engine does not meet
yet can mark elements with `data-known-gap="why"`; component lists cannot be
marked, so fix the engine or leave the page out until it can be.
