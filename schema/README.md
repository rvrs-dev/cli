# The run report

`run-report.schema.json` is the shape of one run as data: what `rvrs <tool> --json` prints, what `rvrs show <id> --json` prints later, and what an account stores. It is JSON Schema 2020-12. `fixtures/` holds one real report per tool, with documentation addresses in place of this device's.

## Rules

- `schema` is the version, `1` today. Within a version fields are only added. A change that would break a reader raises the number.
- A reader ignores fields it doesn't know and keeps values it doesn't know as they are: a new `tool`, a new `origin.kind`, a new `outcome`.
- Every figure is optional. `measured`, `hops`, `records`, `sources`, `probe` and `mtu` say what a run found, and a run from a small device finds less than one from a laptop.
- `id` is unique on the install that made the run. An account tells runs apart by the install and this id.
- `measured` and `hops` are what was measured. `interpretation` holds inferences only, each a sentence that says it is one.

## Where it is checked

`cargo test` checks that every field of a report and of each fixture is one the schema names. The rvrs API reads the same fixtures against its own copy of the shape.
