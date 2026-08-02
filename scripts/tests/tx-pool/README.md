# `check-tx-pool.py` fixtures

Cases for `scripts/check-tx-pool.py --self-test`. Each `.rs.txt` file declares
what it expects on its first line:

    // expect: clean              — no findings
    // expect: violations         — findings on exactly the lines marked
                                    `//~ VIOLATION`, and no others
    // expect: error:<substring>  — a hard `Failure` whose message contains it

Expected violations are marked inline rather than listed as line numbers in the
header, so adding a comment to a fixture cannot silently invalidate it.

They are `.rs.txt`, not `.rs`, and live outside `crates/`/`apps/`, so the real
run's globs never pick them up — several of them are deliberately broken and
would fail the guard itself.

These exist because the guard can fail *silently*. A bug in span detection or in
the signature parser makes it pass everything, which looks exactly like success.
Two such bugs were found while writing it: a generic `fn` head the parser
skipped, and a shadowed name that only broke the error paths. Neither was
visible from the real tree, which is green by design.
