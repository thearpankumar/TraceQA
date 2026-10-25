# Buggy Shop

A small shop page where the right answer is known. TraceQA is tested against it.

```
python fixtures/buggy-shop/serve.py          # http://127.0.0.1:4173/
```

The payment frame is served from the next port (4174), which makes it a
different origin.

| File | What it is |
|---|---|
| `site/` | The shop. Plain HTML, CSS and JavaScript, no build step. |
| `third-party/` | The page inside the cross-origin payment frame. |
| `manifest.yaml` | Every interactive component the page has. `bench/run.py` scores TraceQA against it, and `traceqa verify --manifest` checks a run against it. |

Each hard case is marked with a comment in `site/index.html`: a grid of 12 cards,
shadow DOM, same-origin and cross-origin frames, a list that loads on scroll,
hidden and disabled controls, an icon-only button, clickable elements with no
role, labels that are not English, a mobile-only menu button, and a dialog that
does not exist until a button is clicked.

When you change the shop, change `manifest.yaml` to match.

The name says "buggy" because planted bugs come next, for the features that
find bugs. Today it only has planted *components*.
