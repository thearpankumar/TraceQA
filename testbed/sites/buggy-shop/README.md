# Buggy Shop

A small shop page where the right answer is known. TraceQA is tested against it.

```
python testbed/sites/buggy-shop/serve.py          # http://127.0.0.1:4173/
```

The payment frame is served from the next port (4174), which makes it a
different origin.

| File | What it is |
|---|---|
| `site/` | The shop. Plain HTML, CSS and JavaScript, no build step. |
| `third-party/` | The page inside the cross-origin payment frame. |
| `manifest.yaml` | Every interactive component the page has. `testbed/bench/run.py` scores TraceQA against it, and `traceqa verify --manifest` checks a run against it. |

Each hard case is marked with a comment in `site/index.html`: a grid of 12 cards,
shadow DOM, same-origin and cross-origin frames, a list that loads on scroll,
hidden and disabled controls, an icon-only button, clickable elements with no
role, labels that are not English, a mobile-only menu button, and a dialog that
does not exist until a button is clicked.

When you change the shop, change `manifest.yaml` to match.

## The demo shop (`/shop/`)

The home page above is read by the extraction bench and the id tests, so it stays as it is. The explorer (`traceqa
explore`) is walked on a second shop that lives beside it, under `/shop/`, where every part of the explorer has
something to do. It is the one site the explorer's tests use. Start a walk at `http://127.0.0.1:4173/shop/`.

| Page | What it has |
|---|---|
| `/shop/` | Home. A header and footer on every page. Two tabs that change what the page shows at the same address ("Details" shows a link). A "Help" button that opens a dialog with two links and a close button, and a "More" button that opens a link. An "Offers" button that opens a dialog only the first time anyone asks, for as long as the server runs (the screen after it cannot be reached again: it is the one a walk has to give up on and list as unreachable). A link covered by an overlay (a click is stopped, so the walk presses a key), a link to another site and a mail link (both left alone as blocked), a link to a page that is gone (a failed read, not a screen), a "Continue" form button and a "Your note" text box (left alone read-only, posted and typed with writes), twelve disabled size buttons (they make the page big), and a promo link whose text changes on every load when the server rotates it (`server.rotate = True`; off by default). |
| `/shop/products.html`, `/shop/product-1..3.html` | Three products, each with an "Add to cart" button (posts to `/shop/api/cart`) and a link back. |
| `/shop/cart.html` | "Clear cart" (posts to `/shop/api/clear`) and a link called "Pay now" (`/shop/pay`): both must be left alone by every walk. |
| `/shop/search.html` | Five fields: "Search products", "Your email" (type email), "Phone number" (type tel), "City" and "Company". What is typed is posted to `/shop/api/typed/LABEL/VALUE`. |
| `/shop/account.html` | "Delete account" (a DELETE to `/shop/api/account`): left alone by every walk. |
| `/shop/about.html`, `team.html`, `terms.html`, `archive.html`, `covered-target.html`, `shipping.html`, `returns.html`, `offer.html`, `specs.html` | Small pages, most with nothing to act on. |
| `/shop/app`, `/shop/app/docs`, `/docs/install`, `/charts`, `/charts/bar`, `/report`, `/about` | A single-page app, linked from the shop home ("Open the app"). Every address is one page (`site/shop/app.html`); the links are anchors that a script takes over, changing the address with the browser history and drawing the route, with no page load, as a framework's router does. The report arrives 2.5 seconds after its link is clicked, as when the next page is fetched before the address changes: a walk that reads at once credits the click with the page before. The page has the shop's header and footer above the app's own menu (so the header is on every page of the shop, and a walk can tell it from the pages' own parts). |
| `/shop/chain/1..12.html` | A bare chain: each page has one "Next" link, and the last one links back to the first. For the limits on depth and screens. Not linked from the shop: a walk of it starts there. |
| `/shop/hang-start.html`, `/shop/hang/1..6` | Six links to pages that never answer (they wait 25 seconds). For the time limit of an action. Not linked from the shop. |
| `/shop/checkout`, `/shop/api/...`, `/shop/pay` | Answer for any method: where the shop's forms and buttons post to. |

The server records every request in `server.requests` as `(method, path)`. A read-only walk must leave only GET and HEAD
in it. `tests/test_shop_demo.py` checks the pages and the server without a browser; `tests/test_explore.py` walks it.
