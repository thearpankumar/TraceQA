# Collector cases

Small pages that test the collector one behaviour at a time. Each element states
what the collector must report about it, so a case is HTML only: adding one
needs no test code.

`collector/tests/test_cases.py` opens every page in Chromium, Firefox and
WebKit and checks every case. Run them with:

```sh
uv run pytest collector
```

## Writing a case

Give the element an `id`, which is how the test finds it in the collector's
output, and a `data-expect` attribute:

```html
<button id="save" data-expect="role=button; name=Save; source=content">Save</button>
<div id="text" data-expect="absent">Plain text</div>
```

`data-expect` holds `key=value` pairs separated by `;`. Only the keys given are
checked. `absent` means the collector must not list the element at all.

| Key | Checks | Example |
|---|---|---|
| `role` | The role | `role=button` |
| `name` | The accessible name. `name=` means no name. | `name=Close dialog` |
| `namelength` | The length of the name | `namelength=200` |
| `source` | Where the name came from: `aria-labelledby`, `aria-label`, `label`, `alt`, `value`, `content`, `title` or `placeholder` | `source=label` |
| `tag` | The tag | `tag=select` |
| `confidence` | `high` or `low` | `confidence=low` |
| `signals` | The evidence for being interactive, in any order | `signals=role,tabindex` |
| `shadow` | Whether it is inside shadow DOM | `shadow=true` |
| `visible`, `enabled`, `inviewport` | State | `visible=false` |
| `covered` | `true`, `false`, or `unchecked` when it was not tested (hidden or off screen) | `covered=true` |
| `coveredby` | The selector of what covers it | `coveredby=#cookie-banner` |
| `checked`, `expanded`, `selected`, `pressed`, `readonly`, `required` | State. A state the collector leaves out counts as `false`. | `checked=true` |

Rules:

- Ids are unique within a page.
- Every element the collector lists on a page must have an expectation. A page
  fails when something is listed that nobody expected, so a false positive is
  caught as surely as a miss.
- Values cannot contain `;`.

## Known gaps

When the collector is known to get a case wrong, keep the expectation that is
correct and add the reason:

```html
<a id="skip" class="visually-hidden" href="#main" data-expect="visible=false"
   data-known-gap="a 1x1 pixel clipped element counts as visible">Skip to content</a>
```

The case is reported as an expected failure, so the gap stays visible without
failing CI. When the collector is fixed and the case starts passing, the test
fails with a request to remove `data-known-gap`, so a fixed gap is never left
marked.

## Pages

| Page | Covers |
|---|---|
| `names.html` | Where a name comes from, in order of precedence, and how it is cleaned |
| `visibility.html` | Every way of being hidden, and being out of the viewport |
| `clickable.html` | Elements that can be clicked without being buttons, and what must not be listed |
| `states.html` | Disabled, checked, expanded, pressed, selected, read-only, required |
| `controls.html` | The role of each kind of form control |
| `structure.html` | Open, nested and closed shadow roots, and slotted content |
| `hostile.html` | A page whose first script replaces `Array.prototype.map`, `getBoundingClientRect`, `getComputedStyle`, `Set.prototype.has` and `String.prototype.trim`. Every element must still be read correctly where the driver can run a script before the page's (the Playwright driver), and the result must carry a `collector_tampered` warning. Not run in Safari, where the WebDriver driver has no such hook. |
| `covered.html` | Covered by a banner, a backdrop or an overlay, and the cases that are not covering: clicks passing through, partial overlap, own content, own label |
