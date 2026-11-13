# Kernel baseline comparison

Recorded configurations: **COMPARABLE**.

Report status: COMPLETE_WITH_FAILURES → COMPLETE_WITH_FAILURES. Minimum verified measured samples: 3.

## Samples and outcomes

| Facts | Stage | Before | After | Percentage blockers |
|---:|---|---|---|---|
| 10000 | compiled/session_mount | measured 1/1 verified (COMPLETE:1); warm-up none/0 | measured 1/1 verified (COMPLETE:1); warm-up none/0 | fewer than 3 verified measured samples |
| 10000 | full_compile | measured 3/3 verified (COMPLETE:3); warm-up none/0 | measured 3/3 verified (COMPLETE:3); warm-up none/0 | none |
| 10000 | shacl/focus_core_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 10000 | shacl/focus_core_pass | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 10000 | shacl/focus_nonlocal_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 100000 | compiled/session_mount | measured 1/1 verified (COMPLETE:1); warm-up none/0 | measured 1/1 verified (COMPLETE:1); warm-up none/0 | fewer than 3 verified measured samples |
| 100000 | full_compile | measured 3/3 verified (COMPLETE:3); warm-up none/0 | measured 3/3 verified (COMPLETE:3); warm-up none/0 | none |
| 100000 | shacl/focus_core_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 100000 | shacl/focus_core_pass | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 100000 | shacl/focus_nonlocal_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 1000000 | compiled/session_mount | measured 1/1 verified (COMPLETE:1); warm-up none/0 | measured 1/1 verified (COMPLETE:1); warm-up none/0 | fewer than 3 verified measured samples |
| 1000000 | full_compile | measured 3/3 verified (COMPLETE:3); warm-up none/0 | measured 3/3 verified (COMPLETE:3); warm-up none/0 | none |
| 1000000 | shacl/focus_core_fail | measured 0/3 verified (SKIPPED:3); warm-up BUDGET_EXHAUSTED:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | failed, skipped, unverified or missing planned samples (including warm-ups); fewer than 3 verified measured samples |
| 1000000 | shacl/focus_core_pass | measured 0/3 verified (SKIPPED:3); warm-up BUDGET_EXHAUSTED:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | failed, skipped, unverified or missing planned samples (including warm-ups); fewer than 3 verified measured samples |
| 1000000 | shacl/focus_nonlocal_fail | measured 0/3 verified (SKIPPED:3); warm-up BUDGET_EXHAUSTED:1/1 | measured 0/3 verified (SKIPPED:3); warm-up BUDGET_EXHAUSTED:1/1 | failed, skipped, unverified or missing planned samples (including warm-ups); fewer than 3 verified measured samples |

## Verified measured-sample medians

Change = (after / before − 1) × 100; failure durations and warm-ups are excluded.

| Facts | Stage / metric | Before | After | Change |
|---:|---|---:|---:|---|
| 10000 | compiled/session_mount / end_to_end_seconds | 0.00543174 s (n=1) | 0.0106344 s (n=1) | — |
| 10000 | compiled/session_mount / mount_metrics.elapsed_seconds | 0.00354927 s (n=1) | 0.00800333 s (n=1) | — |
| 10000 | compiled/session_mount / mount_metrics.snapshot_bytes | 1.34375 MiB (n=1) | 1.34375 MiB (n=1) | — |
| 10000 | compiled/session_mount / mount_metrics.copied_bytes | 1.34375 MiB (n=1) | 1.34375 MiB (n=1) | — |
| 10000 | full_compile / end_to_end_seconds | 0.0867788 s (n=3) | 0.0979952 s (n=3) | +12.93% |
| 10000 | full_compile / api_metrics.execution_seconds | 0.0839259 s (n=3) | 0.0938972 s (n=3) | +11.88% |
| 10000 | full_compile / api_metrics.parse_dictionary_seconds | 0.028923 s (n=3) | 0.0280734 s (n=3) | -2.94% |
| 10000 | full_compile / api_metrics.index_seconds | 0.0359665 s (n=3) | 0.0429478 s (n=3) | +19.41% |
| 10000 | full_compile / api_metrics.engine_seconds | 0.0648896 s (n=3) | 0.0727109 s (n=3) | +12.05% |
| 10000 | full_compile / api_metrics.peak_rss_bytes | 18.7422 MiB (n=3) | 19.1992 MiB (n=3) | +2.44% |
| 10000 | full_compile / api_metrics.supervisor_peak_rss_bytes | 6.64453 MiB (n=3) | 7.08203 MiB (n=3) | +6.58% |
| 10000 | full_compile / api_metrics.peak_scratch_bytes | 2.49965 MiB (n=3) | 2.49965 MiB (n=3) | -0.00% |
| 10000 | full_compile / process_wait4_peak_rss_bytes | 18.375 MiB (n=3) | 18.375 MiB (n=3) | +0.00% |
| 10000 | shacl/focus_core_fail / end_to_end_seconds | 0.0422268 s (n=3) | 0.0117377 s (n=3) | -72.20% |
| 10000 | shacl/focus_core_fail / api_metrics.execution_seconds | 0.0416731 s (n=3) | 0.0111109 s (n=3) | -73.34% |
| 10000 | shacl/focus_core_fail / api_metrics.peak_rss_bytes | 31.3203 MiB (n=3) | 17.8711 MiB (n=3) | -42.94% |
| 10000 | shacl/focus_core_fail / api_metrics.supervisor_peak_rss_bytes | 6.82031 MiB (n=3) | 7.07422 MiB (n=3) | +3.72% |
| 10000 | shacl/focus_core_fail / api_metrics.peak_scratch_bytes | 0.0035677 MiB (n=3) | 0.00358295 MiB (n=3) | +0.43% |
| 10000 | shacl/focus_core_pass / end_to_end_seconds | 0.0828666 s (n=3) | 0.0114561 s (n=3) | -86.18% |
| 10000 | shacl/focus_core_pass / api_metrics.execution_seconds | 0.0819908 s (n=3) | 0.0109585 s (n=3) | -86.63% |
| 10000 | shacl/focus_core_pass / api_metrics.peak_rss_bytes | 30.8867 MiB (n=3) | 17.7852 MiB (n=3) | -42.42% |
| 10000 | shacl/focus_core_pass / api_metrics.supervisor_peak_rss_bytes | 6.79297 MiB (n=3) | 7.05078 MiB (n=3) | +3.80% |
| 10000 | shacl/focus_core_pass / api_metrics.peak_scratch_bytes | 0.00336838 MiB (n=3) | 0.00338364 MiB (n=3) | +0.45% |
| 10000 | shacl/focus_nonlocal_fail / end_to_end_seconds | 0.0621829 s (n=3) | 0.0525233 s (n=3) | -15.53% |
| 10000 | shacl/focus_nonlocal_fail / api_metrics.execution_seconds | 0.0616448 s (n=3) | 0.0517554 s (n=3) | -16.04% |
| 10000 | shacl/focus_nonlocal_fail / api_metrics.peak_rss_bytes | 32.0273 MiB (n=3) | 32.9609 MiB (n=3) | +2.91% |
| 10000 | shacl/focus_nonlocal_fail / api_metrics.supervisor_peak_rss_bytes | 6.83203 MiB (n=3) | 7.08984 MiB (n=3) | +3.77% |
| 10000 | shacl/focus_nonlocal_fail / api_metrics.peak_scratch_bytes | 0.00368977 MiB (n=3) | 0.00370979 MiB (n=3) | +0.54% |
| 100000 | compiled/session_mount / end_to_end_seconds | 0.0418185 s (n=1) | 0.034365 s (n=1) | — |
| 100000 | compiled/session_mount / mount_metrics.elapsed_seconds | 0.0390322 s (n=1) | 0.0326331 s (n=1) | — |
| 100000 | compiled/session_mount / mount_metrics.snapshot_bytes | 12.75 MiB (n=1) | 12.75 MiB (n=1) | — |
| 100000 | compiled/session_mount / mount_metrics.copied_bytes | 12.75 MiB (n=1) | 12.75 MiB (n=1) | — |
| 100000 | full_compile / end_to_end_seconds | 0.812876 s (n=3) | 0.752383 s (n=3) | -7.44% |
| 100000 | full_compile / api_metrics.execution_seconds | 0.809935 s (n=3) | 0.749575 s (n=3) | -7.45% |
| 100000 | full_compile / api_metrics.parse_dictionary_seconds | 0.308407 s (n=3) | 0.286096 s (n=3) | -7.23% |
| 100000 | full_compile / api_metrics.index_seconds | 0.391044 s (n=3) | 0.359533 s (n=3) | -8.06% |
| 100000 | full_compile / api_metrics.engine_seconds | 0.723289 s (n=3) | 0.655082 s (n=3) | -9.43% |
| 100000 | full_compile / api_metrics.peak_rss_bytes | 45.7461 MiB (n=3) | 46.7188 MiB (n=3) | +2.13% |
| 100000 | full_compile / api_metrics.supervisor_peak_rss_bytes | 6.70703 MiB (n=3) | 7.0625 MiB (n=3) | +5.30% |
| 100000 | full_compile / api_metrics.peak_scratch_bytes | 24.7273 MiB (n=3) | 24.7273 MiB (n=3) | +0.00% |
| 100000 | full_compile / process_wait4_peak_rss_bytes | 39.1211 MiB (n=3) | 39.7148 MiB (n=3) | +1.52% |
| 100000 | shacl/focus_core_fail / end_to_end_seconds | 0.660891 s (n=3) | 0.0113285 s (n=3) | -98.29% |
| 100000 | shacl/focus_core_fail / api_metrics.execution_seconds | 0.660192 s (n=3) | 0.0108927 s (n=3) | -98.35% |
| 100000 | shacl/focus_core_fail / api_metrics.peak_rss_bytes | 157.398 MiB (n=3) | 17.7695 MiB (n=3) | -88.71% |
| 100000 | shacl/focus_core_fail / api_metrics.supervisor_peak_rss_bytes | 7.01172 MiB (n=3) | 7.14062 MiB (n=3) | +1.84% |
| 100000 | shacl/focus_core_fail / api_metrics.peak_scratch_bytes | 11.5811 MiB (n=3) | 0.00358868 MiB (n=3) | -99.97% |
| 100000 | shacl/focus_core_pass / end_to_end_seconds | 0.629812 s (n=3) | 0.0111962 s (n=3) | -98.22% |
| 100000 | shacl/focus_core_pass / api_metrics.execution_seconds | 0.629004 s (n=3) | 0.010713 s (n=3) | -98.30% |
| 100000 | shacl/focus_core_pass / api_metrics.peak_rss_bytes | 157.438 MiB (n=3) | 17.9844 MiB (n=3) | -88.58% |
| 100000 | shacl/focus_core_pass / api_metrics.supervisor_peak_rss_bytes | 6.98047 MiB (n=3) | 7.11719 MiB (n=3) | +1.96% |
| 100000 | shacl/focus_core_pass / api_metrics.peak_scratch_bytes | 12.1085 MiB (n=3) | 0.00338936 MiB (n=3) | -99.97% |
| 100000 | shacl/focus_nonlocal_fail / end_to_end_seconds | 0.679973 s (n=3) | 0.770966 s (n=3) | +13.38% |
| 100000 | shacl/focus_nonlocal_fail / api_metrics.execution_seconds | 0.679392 s (n=3) | 0.770369 s (n=3) | +13.39% |
| 100000 | shacl/focus_nonlocal_fail / api_metrics.peak_rss_bytes | 151.652 MiB (n=3) | 154.66 MiB (n=3) | +1.98% |
| 100000 | shacl/focus_nonlocal_fail / api_metrics.supervisor_peak_rss_bytes | 7.03516 MiB (n=3) | 7.16016 MiB (n=3) | +1.78% |
| 100000 | shacl/focus_nonlocal_fail / api_metrics.peak_scratch_bytes | 12.5889 MiB (n=3) | 12.7608 MiB (n=3) | +1.37% |
| 1000000 | compiled/session_mount / end_to_end_seconds | 0.349021 s (n=1) | 0.333583 s (n=1) | — |
| 1000000 | compiled/session_mount / mount_metrics.elapsed_seconds | 0.344981 s (n=1) | 0.330943 s (n=1) | — |
| 1000000 | compiled/session_mount / mount_metrics.snapshot_bytes | 133.48 MiB (n=1) | 133.48 MiB (n=1) | — |
| 1000000 | compiled/session_mount / mount_metrics.copied_bytes | 133.48 MiB (n=1) | 133.48 MiB (n=1) | — |
| 1000000 | full_compile / end_to_end_seconds | 8.93753 s (n=3) | 8.42306 s (n=3) | -5.76% |
| 1000000 | full_compile / api_metrics.execution_seconds | 8.93155 s (n=3) | 8.41777 s (n=3) | -5.75% |
| 1000000 | full_compile / api_metrics.parse_dictionary_seconds | 3.26578 s (n=3) | 3.28104 s (n=3) | +0.47% |
| 1000000 | full_compile / api_metrics.index_seconds | 4.50316 s (n=3) | 4.33615 s (n=3) | -3.71% |
| 1000000 | full_compile / api_metrics.engine_seconds | 7.83745 s (n=3) | 7.61719 s (n=3) | -2.81% |
| 1000000 | full_compile / api_metrics.peak_rss_bytes | 166.43 MiB (n=3) | 167.539 MiB (n=3) | +0.67% |
| 1000000 | full_compile / api_metrics.supervisor_peak_rss_bytes | 6.87109 MiB (n=3) | 7.13672 MiB (n=3) | +3.87% |
| 1000000 | full_compile / api_metrics.peak_scratch_bytes | 368.821 MiB (n=3) | 373.712 MiB (n=3) | +1.33% |
| 1000000 | full_compile / process_wait4_peak_rss_bytes | 159.574 MiB (n=3) | 160.316 MiB (n=3) | +0.47% |
| 1000000 | shacl/focus_core_fail / end_to_end_seconds | — (n=0) | 0.0115492 s (n=3) | — |
| 1000000 | shacl/focus_core_fail / api_metrics.execution_seconds | — (n=0) | 0.0109647 s (n=3) | — |
| 1000000 | shacl/focus_core_fail / api_metrics.peak_rss_bytes | — (n=0) | 17.7773 MiB (n=3) | — |
| 1000000 | shacl/focus_core_fail / api_metrics.supervisor_peak_rss_bytes | — (n=0) | 7.14844 MiB (n=3) | — |
| 1000000 | shacl/focus_core_fail / api_metrics.peak_scratch_bytes | — (n=0) | 0.00359344 MiB (n=3) | — |
| 1000000 | shacl/focus_core_pass / end_to_end_seconds | — (n=0) | 0.0118508 s (n=3) | — |
| 1000000 | shacl/focus_core_pass / api_metrics.execution_seconds | — (n=0) | 0.011315 s (n=3) | — |
| 1000000 | shacl/focus_core_pass / api_metrics.peak_rss_bytes | — (n=0) | 18.0352 MiB (n=3) | — |
| 1000000 | shacl/focus_core_pass / api_metrics.supervisor_peak_rss_bytes | — (n=0) | 7.125 MiB (n=3) | — |
| 1000000 | shacl/focus_core_pass / api_metrics.peak_scratch_bytes | — (n=0) | 0.00339413 MiB (n=3) | — |
| 1000000 | shacl/focus_nonlocal_fail / end_to_end_seconds | — (n=0) | — (n=0) | — |

## Interpretation

- Matching recorded configuration does not establish the same physical host, storage device, host load or cache state; virtual environments may share resources.
- Percentages are descriptive median changes, not statistical significance. Negative means lower latency or memory; no speed/regression threshold is applied.
- Only COMPLETE samples with oracle.checked=true enter medians. Warm-ups never enter medians; failed, skipped, unverified or missing trials suppress stage percentages.
- Build declarations and rustc_on_path are recorded provenance, not independent proof of how a binary was built. Different engine identities are allowed.
- API RSS, supervisor RSS and wait4 RSS are distinct measurements; do not add them. Engine compile time overlaps parse/index phases; query/SHACL core time is unavailable.
- File and mounted-session reads have different timing boundaries; their configurations are deliberately non-comparable. Session mounts are one-time diagnostic samples and are not hidden in per-request times.
- Baseline and scaling reports are separate study schemas. Scaling comparisons require matching selected phases, driver and harness provenance; build receipt digests and local paths are not execution configuration.
