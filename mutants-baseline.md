# Why each baseline entry survives

`mutants-baseline.txt` is a bare list because `bin/mutants-gate` compares it with
`comm`. This file carries the reasoning, one section per entry. An entry with no
justification here should be treated as an unclosed gap, not as an accepted one.

## `src/executor/pruning.rs:72:16` — `<` to `<=` in `auto_prune`
## `src/executor/pruning.rs:230:24` — `<` to `<=` in `written_recently`

Both compare an age against a duration:

```rust
age < Duration::from_secs(retention.prune_interval_hours * 3600)   // the throttle
age < CACHE_PRUNE_GRACE                                            // the cache grace
```

`<` and `<=` differ only when `age` is exactly equal to the bound, to the
nanosecond. `age` comes from `SystemTime::now().duration_since(mtime)` in both
cases, and that has always advanced some microseconds past the mtime a test set,
so no wall-clock schedule lands on exact equality. There is no test that can
distinguish them from outside the function.

Nor is the distinction worth reaching for: it decides whether a prune happens at
one exact instant, and either answer is correct behaviour.

Recorded rather than papered over, and rather than contorting a test into
something that pretends to cover it.

## Previously here, now killed

`src/executor/pruning.rs:15:60` — `*` to `+` in `CACHE_PRUNE_GRACE`, which made
the grace period 75 seconds instead of 900. It survived because every cache test
backdated with `age_out` (grace + 60s), so an entry read as aged under both
values, and the fresh-entry test wrote at the current instant, which is fresh
under both. Killed by `prune_orphaned_cache_grace_period_is_fifteen_minutes`,
which spares a 5-minute-old entry and removes a 20-minute-old one - two ages that
straddle the wrong value.
