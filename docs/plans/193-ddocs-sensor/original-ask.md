# Original ask

From pm-001 (pij-steady-otter), 2026-11-04, packet job 4, from the #110 reviewer and approved by the prime: add a back-pressure sensor that runs `ddocs validate` over `docs/project/**` and the skill examples (`skills/pij/references/community/`), in `.harness/extensions/checks/` and `just self-check` together. It must work in CI, where ddocs is not installed today (ruling C10: `PIJ_DDOCS`, else PATH); docs-system decides how ddocs gets there in CI, or the sensor reports UNAVAILABLE honestly. It must never pass silently. Prove it fires. Cost: none at rest.
