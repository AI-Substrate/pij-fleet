# Security Policy

## Reporting a vulnerability

**Please do not open a public issue for security problems.**

Report vulnerabilities privately through GitHub's
[private vulnerability reporting](https://github.com/AI-Substrate/pij-fleet/security/advisories/new)
("Security" tab → "Report a vulnerability"). Only the maintainers can see the
report.

Please include:

- what is affected (daemon, CLI, a harness extension, the installer, …) and the
  version or commit;
- steps to reproduce, or a proof of concept;
- the impact you expect (for example: another local user can deliver messages
  as your seat, or read your daemon key).

We aim to acknowledge reports within 7 days and will keep you updated until a
fix ships. We are happy to credit you in the advisory unless you prefer
otherwise.

## Scope notes

pij is a local developer tool. The daemon listens on loopback and authenticates
with a per-boot bearer key stored in your pij state directory; anyone who can
read that file can act as any seat on the machine. Third-party pi extensions run
with your full user privileges — see "Security protocol" in `AGENTS.md` for how
pij vets packages before installing them.

## Supported versions

Only the latest commit on `main` is supported.
