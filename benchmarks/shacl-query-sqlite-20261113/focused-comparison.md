# Kernel baseline comparison

Recorded configurations: **COMPARABLE**.

Report status: COMPLETE_WITH_FAILURES → COMPLETE. Minimum verified measured samples: 3.

## Samples and outcomes

| Facts | Stage | Before | After | Percentage blockers |
|---:|---|---|---|---|
| 10000 | compiled/session_mount | measured 1/1 verified (COMPLETE:1); warm-up none/0 | measured 1/1 verified (COMPLETE:1); warm-up none/0 | fewer than 3 verified measured samples |
| 10000 | full_compile | measured 3/3 verified (COMPLETE:3); warm-up none/0 | measured 3/3 verified (COMPLETE:3); warm-up none/0 | none |
| 10000 | shacl/focus_core_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 10000 | shacl/focus_core_pass | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 10000 | shacl/focus_full_scan_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 10000 | shacl/focus_nonlocal_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 10000 | shacl/focus_path_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 100000 | compiled/session_mount | measured 1/1 verified (COMPLETE:1); warm-up none/0 | measured 1/1 verified (COMPLETE:1); warm-up none/0 | fewer than 3 verified measured samples |
| 100000 | full_compile | measured 3/3 verified (COMPLETE:3); warm-up none/0 | measured 3/3 verified (COMPLETE:3); warm-up none/0 | none |
| 100000 | shacl/focus_core_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 100000 | shacl/focus_core_pass | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 100000 | shacl/focus_full_scan_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 100000 | shacl/focus_nonlocal_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 100000 | shacl/focus_path_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 1000000 | compiled/session_mount | measured 1/1 verified (COMPLETE:1); warm-up none/0 | measured 1/1 verified (COMPLETE:1); warm-up none/0 | fewer than 3 verified measured samples |
| 1000000 | full_compile | measured 3/3 verified (COMPLETE:3); warm-up none/0 | measured 3/3 verified (COMPLETE:3); warm-up none/0 | none |
| 1000000 | shacl/focus_core_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 1000000 | shacl/focus_core_pass | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 1000000 | shacl/focus_full_scan_fail | measured 0/3 verified (SKIPPED:3); warm-up BUDGET_EXHAUSTED:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | failed, skipped, unverified or missing planned samples (including warm-ups); fewer than 3 verified measured samples |
| 1000000 | shacl/focus_nonlocal_fail | measured 0/3 verified (SKIPPED:3); warm-up BUDGET_EXHAUSTED:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | failed, skipped, unverified or missing planned samples (including warm-ups); fewer than 3 verified measured samples |
| 1000000 | shacl/focus_path_fail | measured 0/3 verified (SKIPPED:3); warm-up BUDGET_EXHAUSTED:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | failed, skipped, unverified or missing planned samples (including warm-ups); fewer than 3 verified measured samples |

## Verified measured-sample medians

Change = (after / before − 1) × 100; failure durations and warm-ups are excluded.

| Facts | Stage / metric | Before | After | Change |
|---:|---|---:|---:|---|
| 10000 | compiled/session_mount / end_to_end_seconds | 0.00574433 s (n=1) | 0.011501 s (n=1) | — |
| 10000 | compiled/session_mount / mount_metrics.elapsed_seconds | 0.00405539 s (n=1) | 0.00838818 s (n=1) | — |
| 10000 | compiled/session_mount / mount_metrics.snapshot_bytes | 1.34375 MiB (n=1) | 1.34375 MiB (n=1) | — |
| 10000 | compiled/session_mount / mount_metrics.copied_bytes | 1.34375 MiB (n=1) | 1.34375 MiB (n=1) | — |
| 10000 | full_compile / end_to_end_seconds | 0.0965574 s (n=3) | 0.107488 s (n=3) | +11.32% |
| 10000 | full_compile / api_metrics.execution_seconds | 0.0936262 s (n=3) | 0.104674 s (n=3) | +11.80% |
| 10000 | full_compile / api_metrics.parse_dictionary_seconds | 0.034421 s (n=3) | 0.0370316 s (n=3) | +7.58% |
| 10000 | full_compile / api_metrics.index_seconds | 0.0392809 s (n=3) | 0.0426755 s (n=3) | +8.64% |
| 10000 | full_compile / api_metrics.engine_seconds | 0.0745196 s (n=3) | 0.0790539 s (n=3) | +6.08% |
| 10000 | full_compile / api_metrics.peak_rss_bytes | 18.3633 MiB (n=3) | 19.5469 MiB (n=3) | +6.45% |
| 10000 | full_compile / api_metrics.supervisor_peak_rss_bytes | 6.36719 MiB (n=3) | 6.86328 MiB (n=3) | +7.79% |
| 10000 | full_compile / api_metrics.peak_scratch_bytes | 2.49965 MiB (n=3) | 2.49965 MiB (n=3) | -0.00% |
| 10000 | full_compile / process_wait4_peak_rss_bytes | 18.375 MiB (n=3) | 18.375 MiB (n=3) | +0.00% |
| 10000 | shacl/focus_core_fail / end_to_end_seconds | 0.0113983 s (n=3) | 0.0116764 s (n=3) | +2.44% |
| 10000 | shacl/focus_core_fail / api_metrics.execution_seconds | 0.0107499 s (n=3) | 0.0110607 s (n=3) | +2.89% |
| 10000 | shacl/focus_core_fail / api_metrics.peak_rss_bytes | 16.6094 MiB (n=3) | 18.0234 MiB (n=3) | +8.51% |
| 10000 | shacl/focus_core_fail / api_metrics.supervisor_peak_rss_bytes | 6.32422 MiB (n=3) | 7.32422 MiB (n=3) | +15.81% |
| 10000 | shacl/focus_core_fail / api_metrics.peak_scratch_bytes | 0.00358295 MiB (n=3) | 0.00358295 MiB (n=3) | +0.00% |
| 10000 | shacl/focus_core_pass / end_to_end_seconds | 0.0113151 s (n=3) | 0.0115693 s (n=3) | +2.25% |
| 10000 | shacl/focus_core_pass / api_metrics.execution_seconds | 0.0107887 s (n=3) | 0.0109807 s (n=3) | +1.78% |
| 10000 | shacl/focus_core_pass / api_metrics.peak_rss_bytes | 16.6523 MiB (n=3) | 18.0469 MiB (n=3) | +8.37% |
| 10000 | shacl/focus_core_pass / api_metrics.supervisor_peak_rss_bytes | 6.30469 MiB (n=3) | 7.30469 MiB (n=3) | +15.86% |
| 10000 | shacl/focus_core_pass / api_metrics.peak_scratch_bytes | 0.00338268 MiB (n=3) | 0.00338268 MiB (n=3) | +0.00% |
| 10000 | shacl/focus_full_scan_fail / end_to_end_seconds | 0.0622301 s (n=3) | 0.0215844 s (n=3) | -65.32% |
| 10000 | shacl/focus_full_scan_fail / api_metrics.execution_seconds | 0.0615548 s (n=3) | 0.0209992 s (n=3) | -65.89% |
| 10000 | shacl/focus_full_scan_fail / api_metrics.peak_rss_bytes | 31.8164 MiB (n=3) | 21.3516 MiB (n=3) | -32.89% |
| 10000 | shacl/focus_full_scan_fail / api_metrics.supervisor_peak_rss_bytes | 6.37109 MiB (n=3) | 7.34766 MiB (n=3) | +15.33% |
| 10000 | shacl/focus_full_scan_fail / api_metrics.peak_scratch_bytes | 0.00370502 MiB (n=3) | 0.00370407 MiB (n=3) | -0.03% |
| 10000 | shacl/focus_nonlocal_fail / end_to_end_seconds | 0.0826462 s (n=3) | 0.0115047 s (n=3) | -86.08% |
| 10000 | shacl/focus_nonlocal_fail / api_metrics.execution_seconds | 0.0819462 s (n=3) | 0.0109966 s (n=3) | -86.58% |
| 10000 | shacl/focus_nonlocal_fail / api_metrics.peak_rss_bytes | 31.6328 MiB (n=3) | 20.5195 MiB (n=3) | -35.13% |
| 10000 | shacl/focus_nonlocal_fail / api_metrics.supervisor_peak_rss_bytes | 6.34766 MiB (n=3) | 7.33203 MiB (n=3) | +15.51% |
| 10000 | shacl/focus_nonlocal_fail / api_metrics.peak_scratch_bytes | 0.00371456 MiB (n=3) | 0.00370407 MiB (n=3) | -0.28% |
| 10000 | shacl/focus_path_fail / end_to_end_seconds | 0.0619631 s (n=3) | 0.0124435 s (n=3) | -79.92% |
| 10000 | shacl/focus_path_fail / api_metrics.execution_seconds | 0.0613591 s (n=3) | 0.0110288 s (n=3) | -82.03% |
| 10000 | shacl/focus_path_fail / api_metrics.peak_rss_bytes | 31.793 MiB (n=3) | 20.4414 MiB (n=3) | -35.70% |
| 10000 | shacl/focus_path_fail / api_metrics.supervisor_peak_rss_bytes | 6.36328 MiB (n=3) | 7.33984 MiB (n=3) | +15.35% |
| 10000 | shacl/focus_path_fail / api_metrics.peak_scratch_bytes | 0.00377369 MiB (n=3) | 0.00377369 MiB (n=3) | +0.00% |
| 100000 | compiled/session_mount / end_to_end_seconds | 0.0329509 s (n=1) | 0.0554674 s (n=1) | — |
| 100000 | compiled/session_mount / mount_metrics.elapsed_seconds | 0.0311396 s (n=1) | 0.0536524 s (n=1) | — |
| 100000 | compiled/session_mount / mount_metrics.snapshot_bytes | 12.75 MiB (n=1) | 12.75 MiB (n=1) | — |
| 100000 | compiled/session_mount / mount_metrics.copied_bytes | 12.75 MiB (n=1) | 12.75 MiB (n=1) | — |
| 100000 | full_compile / end_to_end_seconds | 0.810128 s (n=3) | 0.950919 s (n=3) | +17.38% |
| 100000 | full_compile / api_metrics.execution_seconds | 0.807662 s (n=3) | 0.942619 s (n=3) | +16.71% |
| 100000 | full_compile / api_metrics.parse_dictionary_seconds | 0.305481 s (n=3) | 0.319477 s (n=3) | +4.58% |
| 100000 | full_compile / api_metrics.index_seconds | 0.407004 s (n=3) | 0.416358 s (n=3) | +2.30% |
| 100000 | full_compile / api_metrics.engine_seconds | 0.704246 s (n=3) | 0.75244 s (n=3) | +6.84% |
| 100000 | full_compile / api_metrics.peak_rss_bytes | 45.2422 MiB (n=3) | 46.7656 MiB (n=3) | +3.37% |
| 100000 | full_compile / api_metrics.supervisor_peak_rss_bytes | 6.375 MiB (n=3) | 7.24219 MiB (n=3) | +13.60% |
| 100000 | full_compile / api_metrics.peak_scratch_bytes | 24.7273 MiB (n=3) | 24.7273 MiB (n=3) | +0.00% |
| 100000 | full_compile / process_wait4_peak_rss_bytes | 39.1094 MiB (n=3) | 39.5859 MiB (n=3) | +1.22% |
| 100000 | shacl/focus_core_fail / end_to_end_seconds | 0.0113875 s (n=3) | 0.0114076 s (n=3) | +0.18% |
| 100000 | shacl/focus_core_fail / api_metrics.execution_seconds | 0.0108964 s (n=3) | 0.0108183 s (n=3) | -0.72% |
| 100000 | shacl/focus_core_fail / api_metrics.peak_rss_bytes | 16.8281 MiB (n=3) | 17.8477 MiB (n=3) | +6.06% |
| 100000 | shacl/focus_core_fail / api_metrics.supervisor_peak_rss_bytes | 6.33594 MiB (n=3) | 7.01562 MiB (n=3) | +10.73% |
| 100000 | shacl/focus_core_fail / api_metrics.peak_scratch_bytes | 0.00358868 MiB (n=3) | 0.00358868 MiB (n=3) | +0.00% |
| 100000 | shacl/focus_core_pass / end_to_end_seconds | 0.0111547 s (n=3) | 0.0113523 s (n=3) | +1.77% |
| 100000 | shacl/focus_core_pass / api_metrics.execution_seconds | 0.0106989 s (n=3) | 0.0107844 s (n=3) | +0.80% |
| 100000 | shacl/focus_core_pass / api_metrics.peak_rss_bytes | 16.6562 MiB (n=3) | 17.8125 MiB (n=3) | +6.94% |
| 100000 | shacl/focus_core_pass / api_metrics.supervisor_peak_rss_bytes | 6.30859 MiB (n=3) | 6.98828 MiB (n=3) | +10.77% |
| 100000 | shacl/focus_core_pass / api_metrics.peak_scratch_bytes | 0.00338936 MiB (n=3) | 0.00338936 MiB (n=3) | +0.00% |
| 100000 | shacl/focus_full_scan_fail / end_to_end_seconds | 0.822955 s (n=3) | 0.174357 s (n=3) | -78.81% |
| 100000 | shacl/focus_full_scan_fail / api_metrics.execution_seconds | 0.822337 s (n=3) | 0.173437 s (n=3) | -78.91% |
| 100000 | shacl/focus_full_scan_fail / api_metrics.peak_rss_bytes | 153.594 MiB (n=3) | 25.4102 MiB (n=3) | -83.46% |
| 100000 | shacl/focus_full_scan_fail / api_metrics.supervisor_peak_rss_bytes | 6.38672 MiB (n=3) | 7.05078 MiB (n=3) | +10.40% |
| 100000 | shacl/focus_full_scan_fail / api_metrics.peak_scratch_bytes | 11.9608 MiB (n=3) | 0.00371933 MiB (n=3) | -99.97% |
| 100000 | shacl/focus_nonlocal_fail / end_to_end_seconds | 0.823077 s (n=3) | 0.0340133 s (n=3) | -95.87% |
| 100000 | shacl/focus_nonlocal_fail / api_metrics.execution_seconds | 0.821037 s (n=3) | 0.0330304 s (n=3) | -95.98% |
| 100000 | shacl/focus_nonlocal_fail / api_metrics.peak_rss_bytes | 153.438 MiB (n=3) | 24.1484 MiB (n=3) | -84.26% |
| 100000 | shacl/focus_nonlocal_fail / api_metrics.supervisor_peak_rss_bytes | 6.35938 MiB (n=3) | 7.02734 MiB (n=3) | +10.50% |
| 100000 | shacl/focus_nonlocal_fail / api_metrics.peak_scratch_bytes | 12.6483 MiB (n=3) | 0.00371456 MiB (n=3) | -99.97% |
| 100000 | shacl/focus_path_fail / end_to_end_seconds | 0.801615 s (n=3) | 0.032256 s (n=3) | -95.98% |
| 100000 | shacl/focus_path_fail / api_metrics.execution_seconds | 0.800924 s (n=3) | 0.0312592 s (n=3) | -96.10% |
| 100000 | shacl/focus_path_fail / api_metrics.peak_rss_bytes | 153.758 MiB (n=3) | 23.9766 MiB (n=3) | -84.41% |
| 100000 | shacl/focus_path_fail / api_metrics.supervisor_peak_rss_bytes | 6.37891 MiB (n=3) | 7.03516 MiB (n=3) | +10.29% |
| 100000 | shacl/focus_path_fail / api_metrics.peak_scratch_bytes | 12.4178 MiB (n=3) | 0.00378418 MiB (n=3) | -99.97% |
| 1000000 | compiled/session_mount / end_to_end_seconds | 0.334094 s (n=1) | 0.448225 s (n=1) | — |
| 1000000 | compiled/session_mount / mount_metrics.elapsed_seconds | 0.330861 s (n=1) | 0.443887 s (n=1) | — |
| 1000000 | compiled/session_mount / mount_metrics.snapshot_bytes | 133.48 MiB (n=1) | 133.48 MiB (n=1) | — |
| 1000000 | compiled/session_mount / mount_metrics.copied_bytes | 133.48 MiB (n=1) | 133.48 MiB (n=1) | — |
| 1000000 | full_compile / end_to_end_seconds | 8.69152 s (n=3) | 7.935 s (n=3) | -8.70% |
| 1000000 | full_compile / api_metrics.execution_seconds | 8.68666 s (n=3) | 7.93233 s (n=3) | -8.68% |
| 1000000 | full_compile / api_metrics.parse_dictionary_seconds | 3.5144 s (n=3) | 3.06295 s (n=3) | -12.85% |
| 1000000 | full_compile / api_metrics.index_seconds | 4.42211 s (n=3) | 4.12925 s (n=3) | -6.62% |
| 1000000 | full_compile / api_metrics.engine_seconds | 7.87885 s (n=3) | 7.1922 s (n=3) | -8.72% |
| 1000000 | full_compile / api_metrics.peak_rss_bytes | 166.133 MiB (n=3) | 167.059 MiB (n=3) | +0.56% |
| 1000000 | full_compile / api_metrics.supervisor_peak_rss_bytes | 6.5 MiB (n=3) | 7.00391 MiB (n=3) | +7.75% |
| 1000000 | full_compile / api_metrics.peak_scratch_bytes | 375.146 MiB (n=3) | 363.892 MiB (n=3) | -3.00% |
| 1000000 | full_compile / process_wait4_peak_rss_bytes | 159.477 MiB (n=3) | 160.004 MiB (n=3) | +0.33% |
| 1000000 | shacl/focus_core_fail / end_to_end_seconds | 0.0115639 s (n=3) | 0.0114311 s (n=3) | -1.15% |
| 1000000 | shacl/focus_core_fail / api_metrics.execution_seconds | 0.0110021 s (n=3) | 0.0109007 s (n=3) | -0.92% |
| 1000000 | shacl/focus_core_fail / api_metrics.peak_rss_bytes | 16.8242 MiB (n=3) | 17.7539 MiB (n=3) | +5.53% |
| 1000000 | shacl/focus_core_fail / api_metrics.supervisor_peak_rss_bytes | 6.53125 MiB (n=3) | 7.07031 MiB (n=3) | +8.25% |
| 1000000 | shacl/focus_core_fail / api_metrics.peak_scratch_bytes | 0.00359344 MiB (n=3) | 0.00359344 MiB (n=3) | +0.00% |
| 1000000 | shacl/focus_core_pass / end_to_end_seconds | 0.0118167 s (n=3) | 0.0118151 s (n=3) | -0.01% |
| 1000000 | shacl/focus_core_pass / api_metrics.execution_seconds | 0.0110311 s (n=3) | 0.0113031 s (n=3) | +2.47% |
| 1000000 | shacl/focus_core_pass / api_metrics.peak_rss_bytes | 16.8555 MiB (n=3) | 17.7227 MiB (n=3) | +5.14% |
| 1000000 | shacl/focus_core_pass / api_metrics.supervisor_peak_rss_bytes | 6.50391 MiB (n=3) | 7.04297 MiB (n=3) | +8.29% |
| 1000000 | shacl/focus_core_pass / api_metrics.peak_scratch_bytes | 0.00339413 MiB (n=3) | 0.00339413 MiB (n=3) | +0.00% |
| 1000000 | shacl/focus_full_scan_fail / end_to_end_seconds | — (n=0) | 1.63509 s (n=3) | — |
| 1000000 | shacl/focus_full_scan_fail / api_metrics.execution_seconds | — (n=0) | 1.63399 s (n=3) | — |
| 1000000 | shacl/focus_full_scan_fail / api_metrics.peak_rss_bytes | — (n=0) | 25.3789 MiB (n=3) | — |
| 1000000 | shacl/focus_full_scan_fail / api_metrics.supervisor_peak_rss_bytes | — (n=0) | 7.10938 MiB (n=3) | — |
| 1000000 | shacl/focus_full_scan_fail / api_metrics.peak_scratch_bytes | — (n=0) | 0.00372505 MiB (n=3) | — |
| 1000000 | shacl/focus_nonlocal_fail / end_to_end_seconds | — (n=0) | 0.143333 s (n=3) | — |
| 1000000 | shacl/focus_nonlocal_fail / api_metrics.execution_seconds | — (n=0) | 0.142702 s (n=3) | — |
| 1000000 | shacl/focus_nonlocal_fail / api_metrics.peak_rss_bytes | — (n=0) | 23.9883 MiB (n=3) | — |
| 1000000 | shacl/focus_nonlocal_fail / api_metrics.supervisor_peak_rss_bytes | — (n=0) | 7.08594 MiB (n=3) | — |
| 1000000 | shacl/focus_nonlocal_fail / api_metrics.peak_scratch_bytes | — (n=0) | 0.00372601 MiB (n=3) | — |
| 1000000 | shacl/focus_path_fail / end_to_end_seconds | — (n=0) | 0.281027 s (n=3) | — |
| 1000000 | shacl/focus_path_fail / api_metrics.execution_seconds | — (n=0) | 0.280433 s (n=3) | — |
| 1000000 | shacl/focus_path_fail / api_metrics.peak_rss_bytes | — (n=0) | 24.2109 MiB (n=3) | — |
| 1000000 | shacl/focus_path_fail / api_metrics.supervisor_peak_rss_bytes | — (n=0) | 7.09375 MiB (n=3) | — |
| 1000000 | shacl/focus_path_fail / api_metrics.peak_scratch_bytes | — (n=0) | 0.00378799 MiB (n=3) | — |

## Interpretation

- Matching recorded configuration does not establish the same physical host, storage device, host load or cache state; virtual environments may share resources.
- Percentages are descriptive median changes, not statistical significance. Negative means lower latency or memory; no speed/regression threshold is applied.
- Only COMPLETE samples with oracle.checked=true enter medians. Warm-ups never enter medians; failed, skipped, unverified or missing trials suppress stage percentages.
- Build declarations and rustc_on_path are recorded provenance, not independent proof of how a binary was built. Different engine identities are allowed.
- API RSS, supervisor RSS and wait4 RSS are distinct measurements; do not add them. Engine compile time overlaps parse/index phases; query/SHACL core time is unavailable.
- File and mounted-session reads have different timing boundaries; their configurations are deliberately non-comparable. Session mounts are one-time diagnostic samples and are not hidden in per-request times.
- Baseline and scaling reports are separate study schemas. Scaling comparisons require matching selected phases, driver and harness provenance; build receipt digests and local paths are not execution configuration.
