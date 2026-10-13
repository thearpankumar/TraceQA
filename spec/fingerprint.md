# Component fingerprint, version 1

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
       "v1" + "\n" + role + "\n" + nameKey + "\n" + pathKey )
```

| Part | How it is built |
|---|---|
| `role` | The element's role as collected. |
| `pathKey` | The element's `path` steps joined with ` > `. Each step is its role (its tag when the role is `generic`), plus its name in quotes when it has one. A step with `repeatCount` of 3 or more loses its name, because the name of one card among many is data. Landmarks keep theirs (`banner`, `main`, `navigation`, `contentinfo`, `complementary`, `region`, `form`, `search`): a page with five named sections has five different paths. `repeatIndex` is always left out, so every card in a grid has the same path. Elements inside a frame get a first step `frame "<frame name, or URL path>"`. |
| `nameKey` | The accessible name, normalised: Unicode NFC, trimmed, inner whitespace collapsed, lower-cased. See the two special cases below. |

### Special case 1: no name

An element with an empty name uses the first of these that exists:
`testid:<data-testid>`, then `id:<id>`, then `<tag[type]>#n`. Here `n` counts
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

A second templated slot in the same card (a title link and a brand link) gets
`*#1`, then `*#2`, so the slots stay separate components.

What this means in practice:

| Page pattern | Result |
|---|---|
| Product cards, each with a title link and a button | The title links are one templated component. The buttons are one component. |
| A footer or menu: a list of links, one per item | Every link is its own component. |
| A list of cards that are each a single link | Every card is its own component. |
| Page sections that repeat but hold different controls | Nothing is templated. |

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
- **Names taken from changing text.** A low-confidence element is named by its
  text. If the page rewrites that text, the id changes with it.
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
