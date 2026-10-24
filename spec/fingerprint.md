# Component fingerprint, version 2

**Status: measured on the Buggy Shop and on four real sites.** `bench/run.py`
gets the same ids in Chromium, Firefox and WebKit, at two viewports, across
repeated runs. The first real site showed that the earlier rule merged lists of
different links into one component, and the card rule below replaced it. It is implemented in `crates/traceqa-engine/src/fingerprint.rs`
and `group.rs`; change this file and that code together.

The fingerprint is the `id` of a component. The same component must get the same
id on the next run and in another browser. The decision cache, the state graph
and bug deduping will all key on it.

## Recipe

```
id = "c_" + first 12 hex characters of SHA-256(
       "v2" + "\n" + role + "\n" + nameKey + "\n" + pathKey )
```

| Part | How it is built |
|---|---|
| `role` | The element's role as collected. |
| `pathKey` | The element's `path` steps joined with ` > `, starting at the innermost *named* step (see below). Each step is its role (its tag when the role is `generic`), plus its name in quotes when it has one. The name is treated exactly like a component name: Unicode NFC, trimmed, inner whitespace collapsed, lower-cased, and then `\` is written as `\\` and `"` as `\"`, so a quote or a ` > ` inside a name can never end the step early or pass for a new step. A step with `repeatCount` of 3 or more loses its name, because the name of one card among many is data. Landmarks keep theirs (`banner`, `main`, `navigation`, `contentinfo`, `complementary`, `region`, `form`, `search`): a page with five named sections has five different paths. `repeatIndex` is always left out, so every card in a grid has the same path. Elements inside a frame get a first step `frame "<frame name, or URL path>"`, with the label normalised and escaped like a step name. |
| `nameKey` | The accessible name, normalised: Unicode NFC, trimmed, inner whitespace collapsed, lower-cased. See the two special cases below. |

### The path starts at the innermost named step

A step is *named* when it contributes a name to the key: the page gave it one and
the repeat rule above did not drop it. The path key keeps the frame step (when
there is one), then every step from the **last named step** down to the element.
Everything above that step is left out. Plain wrappers and unnamed landmarks
change with the layout: on Wikipedia the table of contents sits in
`div > div > div > navigation "Contents"` in a wide window and in
`div > div > main > navigation "Contents"` in a narrow one, and with the whole
path in the id, 48 of 1069 components got a different id between 1280x800 and
390x844. With the rule they get the same id. An element with no named ancestor
keeps its full path.

Measured before the rule was adopted (old and new engine, same pages, Chromium):
the Buggy Shop (37 components), every page in `fixtures/patterns` and
`fixtures/collector-cases`, and the responsive fixture all keep the same
component and instance counts. Wikipedia "Web browser": 1069 components became
1068 (one merge, 0.1%) and the ids that differed between the two viewports fell
from 48 to 0. MDN: 274 components both before and after, and the ids that
differed between the two viewports stay at 2.

The card and template logic is not affected: it looks at the full path.

Trade-off: two elements whose paths differ only above their innermost named step
now share an id, for example the same unnamed section in two different unnamed
`div`s, if each also holds a named landmark of the same name. Pages that label
their landmarks lose nothing; unlabelled pages keep the whole path.

### Special case 1: no name

An element with an empty name uses the first of these that exists:
`testid:<data-testid>`, then `id:<id>`, then `<tag[type]>#n`. The `id` counts
only when it is stable, by the same rule the collector uses for its `#id`
selectors: after trimming, it is not empty, does not start with a digit, `:` or
`_`, has no run of 4 or more digits and no run of 8 or more lower-case hex digits
(`0-9a-f`; upper case does not count). So `search-toggle` is used, while
`:r1:`, `_x` and `radix-12345` are generated per page load and fall through to
`<tag[type]>#n`. `spec/fingerprint-vectors.json` holds the cases; the engine
(`is_stable_attr_id`) and the collector are both tested against it. Here `n` counts
the unnamed elements with the same role at the same place: three icon buttons
side by side are `#0`, `#1` and `#2`, while the single icon button in each card
of a grid is `#0` in every card and so stays one component. It also gets an
`unnamed_interactive_element` warning.

### Special case 2: templated names

Product titles in a grid are twelve links with twelve different names, but they
are one component. Repetition alone does not show that: the links in a footer
repeat too, and each of those is its own control. The engine therefore looks for
*cards*.

An item of a repeated group is a card when both hold:

1. It contains at least 2 interactive elements.
2. At least 3 items of the group contain the same arrangement of elements (the
   same roles at the same places).

Within cards, a *slot* is the element at one place in the arrangement. A slot is
templated, so its `nameKey` becomes `*` and `nameIsTemplate` is true, when it has
at least 3 different names across the cards. Two names are not enough: eleven
"In stock" and one "Sold out" stay two components.

No card is looked for under an ancestor with the role `navigation`, `menu`,
`menubar`, `tablist` or `toolbar`.

In a table the item is the row. Its cells repeat too, but they are its columns,
so a step with the role `cell`, `gridcell`, `columnheader` or `rowheader` is
passed over when looking for the repeated item. The select box of each row of a
table is then one templated component, as the title of each card of a grid is.

A second templated slot in the same card (a title link and a brand link) gets
`*#1`, then `*#2`, so the slots stay separate components.

What this means in practice:

| Page pattern | Result |
|---|---|
| Product cards, each with a title link and a button | The title links are one templated component. The buttons are one component. |
| A footer or menu: a list of links, one per item | Every link is its own component. |
| A list of cards that are each a single link | Every card is its own component. |
| Page sections that repeat but hold different controls | Nothing is templated. |

### Special case 3: names that change

A countdown ("Sale ends in 3:42"), a running total ("5 items in cart") or any
text the page rewrites on its own would give its element a new id every run.
The engine reads the page twice: once before it scrolls through it, and once
after, at least a second later. An element still at the same place, with the
same role and at the same position, whose name differs between the two readings
gets the `nameKey` `~`, and its component has `nameChanges: true`. Its name is
the one read last.

Elements whose names stay put are not affected, so their ids are the same as
before this rule existed and `fingerprintVersion` does not change for it. A name that changes
more slowly than the time between the two readings, a clock showing minutes for
one, is not seen to change.

## Version history

- **1**: first recipe.
- **2**: step names (and frame labels) in the path are normalised and escaped
  like component names, so `Cart  Preview` and `cart preview` give one id and a
  quote in a name cannot alias another path. Every id changes, because the version is part of the hash.
  Unnamed elements with a generated `id` (see special case 1) also
  stopped using it. The path now starts at the innermost named step (above), so
  ids survive a responsive layout that re-parents a named landmark.
  `spec/fingerprint-vectors.json` holds the test vectors.

A change that alters any id raises `fingerprintVersion` (`fingerprint::VERSION`)
and regenerates `fixtures/buggy-shop/golden-ids.json`.

## What is left out on purpose

- Pixel position and size: they change with viewport and browser.
- Class names: build tools generate them.
- State (enabled, checked, expanded): a component keeps its id when it changes state.
- Browser and viewport: the same component has the same id everywhere.

## Instance keys

`key = id + ":" + n`, where `n` counts the component's instances in document
order from 0. A key identifies an instance within one run only. It changes when
a list is reordered.

## Known limits

- **Collisions.** Two different elements with the same role, name and path get
  one id and are reported as two instances.
- **Unnamed elements move.** Their ids depend on their order, so adding an
  unnamed button before another one changes the second one's id.
- **Names that change slowly.** A name the page rewrites while it is read is
  left out of the id (special case 3). One that changes less often than that,
  or only between visits, still changes the id.
- **Generated ids that look stable.** The rule for a stable `id` is a
  heuristic. A framework id such as `react-select-a` passes it, and a chosen id
  such as `item-2024` does not.
- **Re-parenting under a name.** A layout that moves a component into or out of
  a differently named landmark, or that changes the name itself, still changes
  its id. Only unnamed wrappers and landmarks above the innermost named step
  are ignored.
- **Renames.** Changing a button's label changes its id. The old and new ids are
  not linked.
- **Translated pages.** The same page in another language gets different ids,
  because names differ.
- **Cards inside navigation.** A card grid inside a `navigation` landmark will
  not be templated.
- **Single-link cards.** A grid of cards that are each one link gives one
  component per card, not one templated component.
- **Unnamed sections.** Sections without a label share a path, so the same link
  in two unlabelled sections is one component with two instances.

## To settle with real numbers

1. Is 12 hex characters (48 bits) enough to avoid accidental collisions at site scale?
2. Should `data-testid` take priority over the name when both exist? It is more
   stable but most sites do not have it.
3. Is the threshold of 3 cards and 3 names right?
4. How often do real sites produce collisions or unstable paths? One real site is
   not enough to say.
