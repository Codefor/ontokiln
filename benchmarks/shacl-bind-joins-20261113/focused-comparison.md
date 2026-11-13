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
| 10000 | shacl/focus_fanout_join_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 10000 | shacl/focus_full_scan_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 10000 | shacl/focus_nonlocal_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 10000 | shacl/focus_path_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 10000 | shacl/multi_target_path_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 100000 | compiled/session_mount | measured 1/1 verified (COMPLETE:1); warm-up none/0 | measured 1/1 verified (COMPLETE:1); warm-up none/0 | fewer than 3 verified measured samples |
| 100000 | full_compile | measured 3/3 verified (COMPLETE:3); warm-up none/0 | measured 3/3 verified (COMPLETE:3); warm-up none/0 | none |
| 100000 | shacl/focus_core_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 100000 | shacl/focus_core_pass | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 100000 | shacl/focus_fanout_join_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 100000 | shacl/focus_full_scan_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 100000 | shacl/focus_nonlocal_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 100000 | shacl/focus_path_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 100000 | shacl/multi_target_path_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 1000000 | compiled/session_mount | measured 1/1 verified (COMPLETE:1); warm-up none/0 | measured 1/1 verified (COMPLETE:1); warm-up none/0 | fewer than 3 verified measured samples |
| 1000000 | full_compile | measured 3/3 verified (COMPLETE:3); warm-up none/0 | measured 3/3 verified (COMPLETE:3); warm-up none/0 | none |
| 1000000 | shacl/focus_core_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 1000000 | shacl/focus_core_pass | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 1000000 | shacl/focus_fanout_join_fail | measured 0/3 verified (SKIPPED:3); warm-up BUDGET_EXHAUSTED:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | failed, skipped, unverified or missing planned samples (including warm-ups); fewer than 3 verified measured samples |
| 1000000 | shacl/focus_full_scan_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 1000000 | shacl/focus_nonlocal_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 1000000 | shacl/focus_path_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |
| 1000000 | shacl/multi_target_path_fail | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | measured 3/3 verified (COMPLETE:3); warm-up COMPLETE:1/1 | none |

## Verified measured-sample medians

Change = (after / before − 1) × 100; failure durations and warm-ups are excluded.

| Facts | Stage / metric | Before | After | Change |
|---:|---|---:|---:|---|
| 10000 | compiled/session_mount / end_to_end_seconds | 0.00592252 s (n=1) | 0.00563 s (n=1) | — |
| 10000 | compiled/session_mount / mount_metrics.elapsed_seconds | 0.00409673 s (n=1) | 0.0037076 s (n=1) | — |
| 10000 | compiled/session_mount / mount_metrics.snapshot_bytes | 1.34375 MiB (n=1) | 1.34375 MiB (n=1) | — |
| 10000 | compiled/session_mount / mount_metrics.copied_bytes | 1.34375 MiB (n=1) | 1.34375 MiB (n=1) | — |
| 10000 | full_compile / end_to_end_seconds | 0.096691 s (n=3) | 0.0868037 s (n=3) | -10.23% |
| 10000 | full_compile / api_metrics.execution_seconds | 0.0939774 s (n=3) | 0.0839402 s (n=3) | -10.68% |
| 10000 | full_compile / api_metrics.parse_dictionary_seconds | 0.0350567 s (n=3) | 0.030263 s (n=3) | -13.67% |
| 10000 | full_compile / api_metrics.index_seconds | 0.0437769 s (n=3) | 0.0354889 s (n=3) | -18.93% |
| 10000 | full_compile / api_metrics.engine_seconds | 0.0767129 s (n=3) | 0.0657523 s (n=3) | -14.29% |
| 10000 | full_compile / api_metrics.peak_rss_bytes | 18.4492 MiB (n=3) | 18.5039 MiB (n=3) | +0.30% |
| 10000 | full_compile / api_metrics.supervisor_peak_rss_bytes | 6.42578 MiB (n=3) | 6.5 MiB (n=3) | +1.16% |
| 10000 | full_compile / api_metrics.peak_scratch_bytes | 2.49965 MiB (n=3) | 2.49965 MiB (n=3) | +0.00% |
| 10000 | full_compile / process_wait4_peak_rss_bytes | 18.625 MiB (n=3) | 18.625 MiB (n=3) | +0.00% |
| 10000 | shacl/focus_core_fail / end_to_end_seconds | 0.0114642 s (n=3) | 0.0114398 s (n=3) | -0.21% |
| 10000 | shacl/focus_core_fail / api_metrics.execution_seconds | 0.0109722 s (n=3) | 0.0109775 s (n=3) | +0.05% |
| 10000 | shacl/focus_core_fail / api_metrics.peak_rss_bytes | 16.6172 MiB (n=3) | 16.9648 MiB (n=3) | +2.09% |
| 10000 | shacl/focus_core_fail / api_metrics.supervisor_peak_rss_bytes | 6.51172 MiB (n=3) | 6.55078 MiB (n=3) | +0.60% |
| 10000 | shacl/focus_core_fail / api_metrics.peak_scratch_bytes | 0.00358295 MiB (n=3) | 0.00358391 MiB (n=3) | +0.03% |
| 10000 | shacl/focus_core_pass / end_to_end_seconds | 0.0113734 s (n=3) | 0.0111932 s (n=3) | -1.58% |
| 10000 | shacl/focus_core_pass / api_metrics.execution_seconds | 0.0108815 s (n=3) | 0.0107316 s (n=3) | -1.38% |
| 10000 | shacl/focus_core_pass / api_metrics.peak_rss_bytes | 16.8164 MiB (n=3) | 16.8047 MiB (n=3) | -0.07% |
| 10000 | shacl/focus_core_pass / api_metrics.supervisor_peak_rss_bytes | 6.49219 MiB (n=3) | 6.53125 MiB (n=3) | +0.60% |
| 10000 | shacl/focus_core_pass / api_metrics.peak_scratch_bytes | 0.00338268 MiB (n=3) | 0.00338364 MiB (n=3) | +0.03% |
| 10000 | shacl/focus_fanout_join_fail / end_to_end_seconds | 0.0216517 s (n=3) | 0.0127021 s (n=3) | -41.33% |
| 10000 | shacl/focus_fanout_join_fail / api_metrics.execution_seconds | 0.0211371 s (n=3) | 0.0109471 s (n=3) | -48.21% |
| 10000 | shacl/focus_fanout_join_fail / api_metrics.peak_rss_bytes | 19.5898 MiB (n=3) | 19.6562 MiB (n=3) | +0.34% |
| 10000 | shacl/focus_fanout_join_fail / api_metrics.supervisor_peak_rss_bytes | 6.54688 MiB (n=3) | 6.58594 MiB (n=3) | +0.60% |
| 10000 | shacl/focus_fanout_join_fail / api_metrics.peak_scratch_bytes | 0.00372314 MiB (n=3) | 0.00372791 MiB (n=3) | +0.13% |
| 10000 | shacl/focus_full_scan_fail / end_to_end_seconds | 0.0213826 s (n=3) | 0.0215761 s (n=3) | +0.91% |
| 10000 | shacl/focus_full_scan_fail / api_metrics.execution_seconds | 0.0208947 s (n=3) | 0.0210489 s (n=3) | +0.74% |
| 10000 | shacl/focus_full_scan_fail / api_metrics.peak_rss_bytes | 19.7812 MiB (n=3) | 19.9883 MiB (n=3) | +1.05% |
| 10000 | shacl/focus_full_scan_fail / api_metrics.supervisor_peak_rss_bytes | 6.53516 MiB (n=3) | 6.57422 MiB (n=3) | +0.60% |
| 10000 | shacl/focus_full_scan_fail / api_metrics.peak_scratch_bytes | 0.00370026 MiB (n=3) | 0.00370121 MiB (n=3) | +0.03% |
| 10000 | shacl/focus_nonlocal_fail / end_to_end_seconds | 0.0114323 s (n=3) | 0.0114437 s (n=3) | +0.10% |
| 10000 | shacl/focus_nonlocal_fail / api_metrics.execution_seconds | 0.0109789 s (n=3) | 0.0109299 s (n=3) | -0.45% |
| 10000 | shacl/focus_nonlocal_fail / api_metrics.peak_rss_bytes | 18.957 MiB (n=3) | 19.0508 MiB (n=3) | +0.49% |
| 10000 | shacl/focus_nonlocal_fail / api_metrics.supervisor_peak_rss_bytes | 6.51953 MiB (n=3) | 6.55859 MiB (n=3) | +0.60% |
| 10000 | shacl/focus_nonlocal_fail / api_metrics.peak_scratch_bytes | 0.00370407 MiB (n=3) | 0.00370502 MiB (n=3) | +0.03% |
| 10000 | shacl/focus_path_fail / end_to_end_seconds | 0.0114342 s (n=3) | 0.0114675 s (n=3) | +0.29% |
| 10000 | shacl/focus_path_fail / api_metrics.execution_seconds | 0.0109029 s (n=3) | 0.0109636 s (n=3) | +0.56% |
| 10000 | shacl/focus_path_fail / api_metrics.peak_rss_bytes | 18.9336 MiB (n=3) | 18.6445 MiB (n=3) | -1.53% |
| 10000 | shacl/focus_path_fail / api_metrics.supervisor_peak_rss_bytes | 6.52734 MiB (n=3) | 6.56641 MiB (n=3) | +0.60% |
| 10000 | shacl/focus_path_fail / api_metrics.peak_scratch_bytes | 0.00377369 MiB (n=3) | 0.00377464 MiB (n=3) | +0.03% |
| 10000 | shacl/multi_target_path_fail / end_to_end_seconds | 0.0114471 s (n=3) | 0.0116735 s (n=3) | +1.98% |
| 10000 | shacl/multi_target_path_fail / api_metrics.execution_seconds | 0.0109117 s (n=3) | 0.0110456 s (n=3) | +1.23% |
| 10000 | shacl/multi_target_path_fail / api_metrics.peak_rss_bytes | 19.0547 MiB (n=3) | 18.8008 MiB (n=3) | -1.33% |
| 10000 | shacl/multi_target_path_fail / api_metrics.supervisor_peak_rss_bytes | 6.54688 MiB (n=3) | 6.58594 MiB (n=3) | +0.60% |
| 10000 | shacl/multi_target_path_fail / api_metrics.peak_scratch_bytes | 0.00533962 MiB (n=3) | 0.00534153 MiB (n=3) | +0.04% |
| 100000 | compiled/session_mount / end_to_end_seconds | 0.0422807 s (n=1) | 0.0386892 s (n=1) | — |
| 100000 | compiled/session_mount / mount_metrics.elapsed_seconds | 0.0402751 s (n=1) | 0.0364101 s (n=1) | — |
| 100000 | compiled/session_mount / mount_metrics.snapshot_bytes | 12.75 MiB (n=1) | 12.75 MiB (n=1) | — |
| 100000 | compiled/session_mount / mount_metrics.copied_bytes | 12.75 MiB (n=1) | 12.75 MiB (n=1) | — |
| 100000 | full_compile / end_to_end_seconds | 0.867145 s (n=3) | 0.867615 s (n=3) | +0.05% |
| 100000 | full_compile / api_metrics.execution_seconds | 0.86403 s (n=3) | 0.862086 s (n=3) | -0.22% |
| 100000 | full_compile / api_metrics.parse_dictionary_seconds | 0.333935 s (n=3) | 0.312466 s (n=3) | -6.43% |
| 100000 | full_compile / api_metrics.index_seconds | 0.409594 s (n=3) | 0.43475 s (n=3) | +6.14% |
| 100000 | full_compile / api_metrics.engine_seconds | 0.743529 s (n=3) | 0.770208 s (n=3) | +3.59% |
| 100000 | full_compile / api_metrics.peak_rss_bytes | 45.3828 MiB (n=3) | 45.4453 MiB (n=3) | +0.14% |
| 100000 | full_compile / api_metrics.supervisor_peak_rss_bytes | 6.33984 MiB (n=3) | 6.48438 MiB (n=3) | +2.28% |
| 100000 | full_compile / api_metrics.peak_scratch_bytes | 24.7273 MiB (n=3) | 24.7273 MiB (n=3) | +0.00% |
| 100000 | full_compile / process_wait4_peak_rss_bytes | 39.0938 MiB (n=3) | 39.0859 MiB (n=3) | -0.02% |
| 100000 | shacl/focus_core_fail / end_to_end_seconds | 0.0114913 s (n=3) | 0.011782 s (n=3) | +2.53% |
| 100000 | shacl/focus_core_fail / api_metrics.execution_seconds | 0.011042 s (n=3) | 0.011193 s (n=3) | +1.37% |
| 100000 | shacl/focus_core_fail / api_metrics.peak_rss_bytes | 16.7109 MiB (n=3) | 16.9766 MiB (n=3) | +1.59% |
| 100000 | shacl/focus_core_fail / api_metrics.supervisor_peak_rss_bytes | 6.41406 MiB (n=3) | 6.48047 MiB (n=3) | +1.04% |
| 100000 | shacl/focus_core_fail / api_metrics.peak_scratch_bytes | 0.00358868 MiB (n=3) | 0.00358868 MiB (n=3) | +0.00% |
| 100000 | shacl/focus_core_pass / end_to_end_seconds | 0.0118784 s (n=3) | 0.0179782 s (n=3) | +51.35% |
| 100000 | shacl/focus_core_pass / api_metrics.execution_seconds | 0.0112232 s (n=3) | 0.0116913 s (n=3) | +4.17% |
| 100000 | shacl/focus_core_pass / api_metrics.peak_rss_bytes | 16.6836 MiB (n=3) | 16.7656 MiB (n=3) | +0.49% |
| 100000 | shacl/focus_core_pass / api_metrics.supervisor_peak_rss_bytes | 6.38672 MiB (n=3) | 6.45312 MiB (n=3) | +1.04% |
| 100000 | shacl/focus_core_pass / api_metrics.peak_scratch_bytes | 0.00338936 MiB (n=3) | 0.00338936 MiB (n=3) | +0.00% |
| 100000 | shacl/focus_fanout_join_fail / end_to_end_seconds | 1.02877 s (n=3) | 0.0732092 s (n=3) | -92.88% |
| 100000 | shacl/focus_fanout_join_fail / api_metrics.execution_seconds | 1.02452 s (n=3) | 0.0724122 s (n=3) | -92.93% |
| 100000 | shacl/focus_fanout_join_fail / api_metrics.peak_rss_bytes | 23.9336 MiB (n=3) | 24.8164 MiB (n=3) | +3.69% |
| 100000 | shacl/focus_fanout_join_fail / api_metrics.supervisor_peak_rss_bytes | 6.46484 MiB (n=3) | 6.53906 MiB (n=3) | +1.15% |
| 100000 | shacl/focus_fanout_join_fail / api_metrics.peak_scratch_bytes | 0.00453758 MiB (n=3) | 0.00453758 MiB (n=3) | +0.00% |
| 100000 | shacl/focus_full_scan_fail / end_to_end_seconds | 0.163442 s (n=3) | 0.165449 s (n=3) | +1.23% |
| 100000 | shacl/focus_full_scan_fail / api_metrics.execution_seconds | 0.162673 s (n=3) | 0.164801 s (n=3) | +1.31% |
| 100000 | shacl/focus_full_scan_fail / api_metrics.peak_rss_bytes | 23.9141 MiB (n=3) | 24.0312 MiB (n=3) | +0.49% |
| 100000 | shacl/focus_full_scan_fail / api_metrics.supervisor_peak_rss_bytes | 6.4375 MiB (n=3) | 6.51562 MiB (n=3) | +1.21% |
| 100000 | shacl/focus_full_scan_fail / api_metrics.peak_scratch_bytes | 0.00453186 MiB (n=3) | 0.00453186 MiB (n=3) | +0.00% |
| 100000 | shacl/focus_nonlocal_fail / end_to_end_seconds | 0.02165 s (n=3) | 0.0118035 s (n=3) | -45.48% |
| 100000 | shacl/focus_nonlocal_fail / api_metrics.execution_seconds | 0.0211208 s (n=3) | 0.0111476 s (n=3) | -47.22% |
| 100000 | shacl/focus_nonlocal_fail / api_metrics.peak_rss_bytes | 22.4023 MiB (n=3) | 18.8945 MiB (n=3) | -15.66% |
| 100000 | shacl/focus_nonlocal_fail / api_metrics.supervisor_peak_rss_bytes | 6.42578 MiB (n=3) | 6.49219 MiB (n=3) | +1.03% |
| 100000 | shacl/focus_nonlocal_fail / api_metrics.peak_scratch_bytes | 0.00371265 MiB (n=3) | 0.00370979 MiB (n=3) | -0.08% |
| 100000 | shacl/focus_path_fail / end_to_end_seconds | 0.0325809 s (n=3) | 0.0122572 s (n=3) | -62.38% |
| 100000 | shacl/focus_path_fail / api_metrics.execution_seconds | 0.0316175 s (n=3) | 0.0114344 s (n=3) | -63.84% |
| 100000 | shacl/focus_path_fail / api_metrics.peak_rss_bytes | 22.3477 MiB (n=3) | 18.707 MiB (n=3) | -16.29% |
| 100000 | shacl/focus_path_fail / api_metrics.supervisor_peak_rss_bytes | 6.42969 MiB (n=3) | 6.5 MiB (n=3) | +1.09% |
| 100000 | shacl/focus_path_fail / api_metrics.peak_scratch_bytes | 0.00377941 MiB (n=3) | 0.00377941 MiB (n=3) | +0.00% |
| 100000 | shacl/multi_target_path_fail / end_to_end_seconds | 0.0621975 s (n=3) | 0.0119917 s (n=3) | -80.72% |
| 100000 | shacl/multi_target_path_fail / api_metrics.execution_seconds | 0.0615988 s (n=3) | 0.0112906 s (n=3) | -81.67% |
| 100000 | shacl/multi_target_path_fail / api_metrics.peak_rss_bytes | 22.5391 MiB (n=3) | 18.7305 MiB (n=3) | -16.90% |
| 100000 | shacl/multi_target_path_fail / api_metrics.supervisor_peak_rss_bytes | 6.46484 MiB (n=3) | 6.52734 MiB (n=3) | +0.97% |
| 100000 | shacl/multi_target_path_fail / api_metrics.peak_scratch_bytes | 0.00535011 MiB (n=3) | 0.00534534 MiB (n=3) | -0.09% |
| 1000000 | compiled/session_mount / end_to_end_seconds | 0.397169 s (n=1) | 0.336795 s (n=1) | — |
| 1000000 | compiled/session_mount / mount_metrics.elapsed_seconds | 0.389025 s (n=1) | 0.333666 s (n=1) | — |
| 1000000 | compiled/session_mount / mount_metrics.snapshot_bytes | 133.48 MiB (n=1) | 133.48 MiB (n=1) | — |
| 1000000 | compiled/session_mount / mount_metrics.copied_bytes | 133.48 MiB (n=1) | 133.48 MiB (n=1) | — |
| 1000000 | full_compile / end_to_end_seconds | 8.93959 s (n=3) | 8.78427 s (n=3) | -1.74% |
| 1000000 | full_compile / api_metrics.execution_seconds | 8.92942 s (n=3) | 8.78122 s (n=3) | -1.66% |
| 1000000 | full_compile / api_metrics.parse_dictionary_seconds | 3.42255 s (n=3) | 3.26749 s (n=3) | -4.53% |
| 1000000 | full_compile / api_metrics.index_seconds | 4.5281 s (n=3) | 4.60481 s (n=3) | +1.69% |
| 1000000 | full_compile / api_metrics.engine_seconds | 7.95064 s (n=3) | 7.97866 s (n=3) | +0.35% |
| 1000000 | full_compile / api_metrics.peak_rss_bytes | 166.133 MiB (n=3) | 166.094 MiB (n=3) | -0.02% |
| 1000000 | full_compile / api_metrics.supervisor_peak_rss_bytes | 6.51172 MiB (n=3) | 6.55859 MiB (n=3) | +0.72% |
| 1000000 | full_compile / api_metrics.peak_scratch_bytes | 377.56 MiB (n=3) | 367.333 MiB (n=3) | -2.71% |
| 1000000 | full_compile / process_wait4_peak_rss_bytes | 159.461 MiB (n=3) | 159.445 MiB (n=3) | -0.01% |
| 1000000 | shacl/focus_core_fail / end_to_end_seconds | 0.0112612 s (n=3) | 0.0118122 s (n=3) | +4.89% |
| 1000000 | shacl/focus_core_fail / api_metrics.execution_seconds | 0.0108695 s (n=3) | 0.0111109 s (n=3) | +2.22% |
| 1000000 | shacl/focus_core_fail / api_metrics.peak_rss_bytes | 16.8008 MiB (n=3) | 16.9727 MiB (n=3) | +1.02% |
| 1000000 | shacl/focus_core_fail / api_metrics.supervisor_peak_rss_bytes | 6.54297 MiB (n=3) | 6.55859 MiB (n=3) | +0.24% |
| 1000000 | shacl/focus_core_fail / api_metrics.peak_scratch_bytes | 0.00359344 MiB (n=3) | 0.00359344 MiB (n=3) | +0.00% |
| 1000000 | shacl/focus_core_pass / end_to_end_seconds | 0.0112117 s (n=3) | 0.011322 s (n=3) | +0.98% |
| 1000000 | shacl/focus_core_pass / api_metrics.execution_seconds | 0.010818 s (n=3) | 0.0108925 s (n=3) | +0.69% |
| 1000000 | shacl/focus_core_pass / api_metrics.peak_rss_bytes | 16.8711 MiB (n=3) | 17.0781 MiB (n=3) | +1.23% |
| 1000000 | shacl/focus_core_pass / api_metrics.supervisor_peak_rss_bytes | 6.51562 MiB (n=3) | 6.53125 MiB (n=3) | +0.24% |
| 1000000 | shacl/focus_core_pass / api_metrics.peak_scratch_bytes | 0.00339413 MiB (n=3) | 0.00339413 MiB (n=3) | +0.00% |
| 1000000 | shacl/focus_fanout_join_fail / end_to_end_seconds | — (n=0) | 0.558383 s (n=3) | — |
| 1000000 | shacl/focus_fanout_join_fail / api_metrics.execution_seconds | — (n=0) | 0.557379 s (n=3) | — |
| 1000000 | shacl/focus_fanout_join_fail / api_metrics.peak_rss_bytes | — (n=0) | 36.4062 MiB (n=3) | — |
| 1000000 | shacl/focus_fanout_join_fail / api_metrics.supervisor_peak_rss_bytes | — (n=0) | 6.60547 MiB (n=3) | — |
| 1000000 | shacl/focus_fanout_join_fail / api_metrics.peak_scratch_bytes | — (n=0) | 0.00454521 MiB (n=3) | — |
| 1000000 | shacl/focus_full_scan_fail / end_to_end_seconds | 1.53342 s (n=3) | 1.52231 s (n=3) | -0.72% |
| 1000000 | shacl/focus_full_scan_fail / api_metrics.execution_seconds | 1.53218 s (n=3) | 1.52079 s (n=3) | -0.74% |
| 1000000 | shacl/focus_full_scan_fail / api_metrics.peak_rss_bytes | 23.9609 MiB (n=3) | 24.0625 MiB (n=3) | +0.42% |
| 1000000 | shacl/focus_full_scan_fail / api_metrics.supervisor_peak_rss_bytes | 6.58984 MiB (n=3) | 6.57812 MiB (n=3) | -0.18% |
| 1000000 | shacl/focus_full_scan_fail / api_metrics.peak_scratch_bytes | 0.00453949 MiB (n=3) | 0.00453949 MiB (n=3) | +0.00% |
| 1000000 | shacl/focus_nonlocal_fail / end_to_end_seconds | 0.143354 s (n=3) | 0.011498 s (n=3) | -91.98% |
| 1000000 | shacl/focus_nonlocal_fail / api_metrics.execution_seconds | 0.142812 s (n=3) | 0.0109834 s (n=3) | -92.31% |
| 1000000 | shacl/focus_nonlocal_fail / api_metrics.peak_rss_bytes | 22.7109 MiB (n=3) | 19.0234 MiB (n=3) | -16.24% |
| 1000000 | shacl/focus_nonlocal_fail / api_metrics.supervisor_peak_rss_bytes | 6.55859 MiB (n=3) | 6.56641 MiB (n=3) | +0.12% |
| 1000000 | shacl/focus_nonlocal_fail / api_metrics.peak_scratch_bytes | 0.00454044 MiB (n=3) | 0.00371456 MiB (n=3) | -18.19% |
| 1000000 | shacl/focus_path_fail / end_to_end_seconds | 0.328404 s (n=3) | 0.0116454 s (n=3) | -96.45% |
| 1000000 | shacl/focus_path_fail / api_metrics.execution_seconds | 0.325926 s (n=3) | 0.0110895 s (n=3) | -96.60% |
| 1000000 | shacl/focus_path_fail / api_metrics.peak_rss_bytes | 22.6875 MiB (n=3) | 18.7773 MiB (n=3) | -17.23% |
| 1000000 | shacl/focus_path_fail / api_metrics.supervisor_peak_rss_bytes | 6.57422 MiB (n=3) | 6.57422 MiB (n=3) | +0.00% |
| 1000000 | shacl/focus_path_fail / api_metrics.peak_scratch_bytes | 0.00453949 MiB (n=3) | 0.00378418 MiB (n=3) | -16.64% |
| 1000000 | shacl/multi_target_path_fail / end_to_end_seconds | 0.857492 s (n=3) | 0.0114343 s (n=3) | -98.67% |
| 1000000 | shacl/multi_target_path_fail / api_metrics.execution_seconds | 0.856299 s (n=3) | 0.0109396 s (n=3) | -98.72% |
| 1000000 | shacl/multi_target_path_fail / api_metrics.peak_rss_bytes | 22.7852 MiB (n=3) | 18.7891 MiB (n=3) | -17.54% |
| 1000000 | shacl/multi_target_path_fail / api_metrics.supervisor_peak_rss_bytes | 6.60156 MiB (n=3) | 6.59766 MiB (n=3) | -0.06% |
| 1000000 | shacl/multi_target_path_fail / api_metrics.peak_scratch_bytes | 0.00535583 MiB (n=3) | 0.00535107 MiB (n=3) | -0.09% |

## Interpretation

- Matching recorded configuration does not establish the same physical host, storage device, host load or cache state; virtual environments may share resources.
- Percentages are descriptive median changes, not statistical significance. Negative means lower latency or memory; no speed/regression threshold is applied.
- Only COMPLETE samples with oracle.checked=true enter medians. Warm-ups never enter medians; failed, skipped, unverified or missing trials suppress stage percentages.
- Build declarations and rustc_on_path are recorded provenance, not independent proof of how a binary was built. Different engine identities are allowed.
- API RSS, supervisor RSS and wait4 RSS are distinct measurements; do not add them. Engine compile time overlaps parse/index phases; query/SHACL core time is unavailable.
- File and mounted-session reads have different timing boundaries; their configurations are deliberately non-comparable. Session mounts are one-time diagnostic samples and are not hidden in per-request times.
- Baseline and scaling reports are separate study schemas. Scaling comparisons require matching selected phases, driver and harness provenance; build receipt digests and local paths are not execution configuration.
