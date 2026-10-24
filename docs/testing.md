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
CARGO_BUILD_JOBS=2 cargo mutants -p traceqa-engine --config mutants.toml -j 2 -- --lib
```

`mutants.toml` at the repository root limits the run to the two files and sets
`timeout_multiplier = 3`. cargo-mutants only reads `.cargo/mutants.toml` on its own, so pass
`--config mutants.toml` as shown. The run takes about 5 minutes on 8 cores and writes its
report to `mutants.out/` (ignored by git; open `missed.txt` first).

Expected result: 73 mutants, 69 caught, 4 unviable (they do not compile), 0 missed. Before
the tests added with this file the score was 63 caught, 6 missed, 4 unviable. There are no
equivalent mutants to exclude; if one appears, add a line to `exclude_re` in `mutants.toml`
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
