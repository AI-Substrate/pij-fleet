# Building a role on a type

How to make a role your community needs: pick its type, give what the type requires, change a default rule when you must, and start a seat in it.

## Pick the type

A role says what a seat is responsible for; its type says how the seat lives. Read the ten with `pij docs get types` (or `pij directory types`) and pick the one whose lifecycle fits:

| If the holder | Type |
|---|---|
| builds one workstream step, then is compacted | coder |
| judges one piece of work at its exact version, then is compacted (by default) | reviewer |
| does one other piece of work, then is closed | worker |
| owns one area while it matters and keeps what it learns written down | knowledge |
| is the only writer of one shared thing | steward |
| reacts to events and rebuilds from current state, keeping nothing it cannot re-derive | watcher |
| is one person's seat into the fleet | human-window |

prime, pa and project-manager are for the seats that hold those positions. The type grants no authority (`pij docs get concepts#a-type-is-a-label-never-authority`).

When none fits, pick the nearest and say what you wished existed with `--type-suggestion <text>`.

## Write it

Start from a template when one fits; it supplies the type:

    pij directory templates
    pij directory add <id> --from template:<name> --why "<why the community needs it>"

Otherwise give everything yourself:

    pij directory add qa --why "a QA role" --title QA --type reviewer \
      --holders one --rule "A verdict on every PR." \
      --packet "Review each PR at its head."

`add` refuses a role with no type (`E-RS-TYPE-MISSING`, listing the ten) or an unknown one (`E-RS-TYPE-UNKNOWN`).

## Give what the type requires

Some types need a field of the role (`E-RS-TYPE-FIELD` names it):

- knowledge: `--keeps <artefact>`, the graph or file it keeps current.
- steward: `--sole-writer-of <thing>`, what only it writes.

`pij docs get types/<type>#a-role-of-this-type-must-give` lists them for one type.

## Change a default rule

Each type's rules are `fixed` (every role of the type follows it) or `default` (a role may change it with a reason):

    pij directory edit <id> --why "<why>" --type-change <rule>=<what instead, and why>

A fixed rule, an unknown rule or a change with no reason refuses (`E-RS-TYPE-RULE`). `pij docs get types/<type>#rules` shows each rule's id and mode.

## The seed

A seat of the role's type is told its type's seed at start. pij ships one per type; a repo may keep its own in governance:

    pij directory types <type> --seed
    pij directory seed <type> --file <path> --why "<why>"

## Hold it

Assign the role to a seat; the seat is told about it and gets the role's memory index:

    pij directory assign <role> <seat> --why "<why>"

`pij directory` then shows the role, its type and its holder. Roles written before types: `pij directory retype` proposes a type for each, and `--apply --why <text>` writes them.
