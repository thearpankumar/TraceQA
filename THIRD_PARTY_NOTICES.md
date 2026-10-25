# Third-party notices

TraceQA is licensed under the Apache License 2.0 (`LICENSE`). The Rust, Python and npm dependencies it is
built from carry their own licences; `deny.toml` lists the ones the Rust tree may contain, and the other
packages' licences are in their own distributions.

## axe-core (optional, downloaded on request, not included)

| | |
|---|---|
| Project | axe-core, by Deque Systems, Inc. |
| Licence | Mozilla Public License 2.0 (MPL-2.0), a file-level copyleft licence |
| Source | https://github.com/dequelabs/axe-core |
| Package | https://www.npmjs.com/package/axe-core |
| Version TraceQA uses | 4.14.0 |

**axe-core is not part of TraceQA.** It is not in this repository, in the `traceqa` binary, in the release archives
or in the Python and npm packages. `traceqa install --axe` is the only thing that fetches it, and only when a user
runs that command:

1. It downloads `https://registry.npmjs.org/axe-core/-/axe-core-4.14.0.tgz` from the official npm registry to the
   user's own machine.
2. It checks the tarball's SHA-512 against the value pinned in the program
   (`sha512-9WTZxEjsZ7b13TH8JPmbV2z8CHbl80/2hm3XPEG4JgNdQLK81IBRXmSxHfMAOkSqQeRxT/0dwNDz2GOm3zzpcQ==`, the registry's
   `dist.integrity`), and refuses anything else.
3. It extracts only `package/axe.min.js`, `package/LICENSE` and, if present, `package/LICENSE-3RD-PARTY.txt`, without
   changing them, into `~/.traceqa/axe/4.14.0/` (or `$TRACEQA_HOME/axe/4.14.0/`).
4. It prints the licence, the location of the source and that the file is unmodified.

`traceqa extract --axe` then sends the unmodified script to the browser being tested, in memory, through the browser
driver. TraceQA's own code is not a modification of axe-core's files: it only runs them and reads their results.

The MPL-2.0 licence text is the `LICENSE` file installed next to `axe.min.js`, and also at
https://www.mozilla.org/en-US/MPL/2.0/. If TraceQA ever bundled axe-core, MPL-2.0 would require the licence text and
the notices inside the file to be kept and recipients to be told where the source is (the address above), as the
Mozilla MPL 2.0 FAQ describes (https://www.mozilla.org/en-US/MPL/2.0/FAQ/, questions 7 to 9). That is why a release
must not include it.

axe-core's own third-party notices, if the package has any, are installed as `LICENSE-3RD-PARTY.txt`.
