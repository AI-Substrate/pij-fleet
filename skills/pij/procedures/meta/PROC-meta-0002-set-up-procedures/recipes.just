
# Procedures (pij PRD 005): run, list, write and check them. `just proc help`.
[positional-arguments]
proc *ARGS:
    pij proc "$@"

# The procedures drift check; add it to this repo's gate.
procedures-check:
    pij proc check
