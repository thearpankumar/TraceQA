# Testing

## Mutation testing of the component id

The component id recipe (`crates/traceqa-engine/src/fingerprint.rs`) and the grouping that
uses it (`crates/traceqa-engine/src/group.rs`) decide whether a control keeps its id from
one run to the next, so their tests are checked with
[cargo-mutants](https://mutants.rs): it changes the code one small step at a time (`>=` to
`<`, `-` to `+`, a function body to a default) and expects at least one test to fail for each
change.

Install once with `cargo install cargo-mutants`, then run it alone on an idle machine
(a busy machine makes tests slow and turns real catches into timeouts):

```sh
CARGO_BUILD_JOBS=2 cargo mutants -p traceqa-engine -j 2 -- --lib
```

`.cargo/mutants.toml` is read automatically. It limits the run to the two files and sets
the time each mutant's tests may take: 5 times the baseline, never less than 120 seconds
(`minimum_test_timeout`). The baseline is the whole `traceqa-engine --lib` test run, including
the tests that start processes, and a loaded machine can be several times slower than an idle
one; with cargo-mutants' default floor of 20 seconds that produced spurious timeouts. A real
hang (a mutant that loops forever) is still caught, after 120 seconds, and shows as exit code 3
and a line in `timeout.txt`. The run takes about 5 minutes on 8 cores and writes its
report to `mutants.out/` (ignored by git; open `missed.txt` first).

Expected result: 73 mutants, 69 caught, 4 unviable (they do not compile), 0 missed. Before
the tests added with this file the score was 63 caught, 6 missed, 4 unviable. There are no
equivalent mutants to exclude; if one appears, add a line to `exclude_re` in `.cargo/mutants.toml`
with a comment saying why no test can tell it apart.

Exit codes ([reference](https://mutants.rs/exit-codes.html)):

| Code | Meaning |
| ---- | ------- |
| 0 | every mutant was caught |
| 1 | usage error |
| 2 | some mutants were missed: add a test that fails for them |
| 3 | some tests timed out: look at the load on the machine, then at the mutant |
| 4 | the unmutated baseline fails: fix the tests first |

A missed mutant is either a missing test or a sign that the code does nothing useful there;
read the line before deciding.

## Generated page objects (`traceqa codegen`)

`tests/test_codegen.py` reads the Buggy Shop in Chromium, Firefox and WebKit (and Chromium at
390x844), writes page objects in both languages and runs them against the live shop. Every
property must find exactly as many elements as that browser's `components.json` recorded. The
Python object runs with Playwright's sync API in the test itself. The TypeScript object runs
with `@playwright/test` in `tests/codegen_ts` (`shop.spec.ts`, three projects), started by the
test through `node node_modules/@playwright/test/cli.js test`.

```sh
uv run pytest tests/test_codegen.py
```

`tests/codegen_ts/package.json` pins `@playwright/test` to the exact version of Playwright in
`uv.lock`, so both languages use the browsers `uv run playwright install chromium firefox webkit`
put in the cache; a test fails when the two versions differ, so update them together (change the
version in `package.json`, run `npm install --package-lock-only` there, and `uv lock`). The test
runs `npm ci` in that folder when `node_modules` is missing; CI does it in its own step. Node 24
or newer is needed (`module.stripTypeScriptTypes`, used by `record.mjs`, which loads a generated
page object with a stand-in `page` to prove that hostile page text stays inside string literals).

A failure here usually means the engine and Playwright disagree about a role or a name. Fix the
engine (`crates/traceqa-engine/src/locator.rs`), not the test.
