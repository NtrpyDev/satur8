# Satur8 Benchmarks

Benchmark datasets live in timestamped directories. Each dataset should keep:

- `runs.csv` - one parsed row per scored run.
- `runs.jsonl` - the same parsed rows, JSON Lines format for site tooling.
- `summary.json` - environment metadata, medians, paired deltas, and notes.
- `raw-console/` - raw pasted benchmark output used to create each row.
- `telemetry-*.txt` - optional system snapshots captured during the session.

## CS2 KWin Cost Protocol

Use one unscored OFF warm-up, then seven alternating scored pairs:

```text
OFF warm-up, OFF1, ON1, OFF2, ON2, OFF3, ON3, OFF4, ON4, OFF5, ON5, OFF6, ON6, OFF7, ON7
```

For ON runs, use Satur8's KWin effect at saturation `1.75` unless the test
metadata says otherwise. Report the all-run medians, a clean-run sensitivity
check for rows with `frames_excluded == 0`, and paired median deltas. If these
disagree, treat the result as run-to-run noise instead of claiming a precise
per-frame cost.

## Restart-Per-Run Automation

For cleaner data, use the local PC1 automation harness:

```sh
scripts/cs2-fps-benchmark-automation.py --pairs 7 --saturation 1.75
```

The script sets Satur8 OFF/ON, launches CS2, navigates to the Workshop benchmark
map, clicks GO, waits for VProf, copies or OCRs the console result, catalogs raw
and parsed output, types `quit`, and relaunches CS2 for the next attempt.

Default acceptance rules are strict:

- `frames_excluded == 0`
- `one_second_intervals == 129`

Rejected attempts are saved in `rejected-runs.csv` / `rejected-runs.jsonl` but
do not count toward the requested seven accepted pairs. Pair order defaults to
`flip`, alternating OFF/ON and ON/OFF by pair to reduce time-order bias.
