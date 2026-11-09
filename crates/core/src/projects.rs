//! `pij project`: create, list, read or update governed projects (F156).

/// The verb's help; `--help` needs no daemon.
pub const HELP: &str = "\
pij project: create, list, read or update governed projects.

Usage:
  pij project create <name> [--repo <path>] [--plan <path>] [--prime <seat>]
  pij project list
  pij project show <slug>
  pij project set <slug> [--description <text>] [--repo <path>] [--plan <path>] [--prime <seat>]

create makes a project from <name>'s slug (a taken slug gets -2, -3, ...).
--repo is its main checkout (default: the caller's folder), --plan its plan
file, and --prime the seat designated as its prime (C24: the designation is
the only prime authority; set it later with `pij project set <slug> --prime`).
";
