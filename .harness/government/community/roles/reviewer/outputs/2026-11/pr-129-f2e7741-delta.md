reviewer: pass-with-notes (head f2e7741dc4801a5824d08d66206468ef9070bc41)

The complete 314da13..f2e7741 delta is two files. The all-state jobs_by_dedupe index covers the recovery equality keys without changing admission evidence. Extracting RECOVER_TAKEN_SQL only reorders equivalent AND predicates; F001-F003 closure and the prior correctness/proof notes stand.

Exact-head CI run [42186187295](https://github.com/AI-Substrate/pij-fleet/actions/runs/42186187295) passes all four jobs. JUnit artifact 13487483367 confirms boot_recovery_probes_are_indexed_searches and both restart/store recovery cases ran and passed; 1969 tests passed overall. The new guard explains the actual production SQL and requires the named indexes, SEARCH for each evidence probe, and no SCAN except the stranded-claims partial index.

No new correctness finding. Worktree clean/frozen; diff --check passed. CI/source only, no local builds/tests or reviewer mutation run. The reported sqlite3 red/green receipt was not independently rerun. Performance's P001 disposition remains separately owned.
