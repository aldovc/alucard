# Engineering policy

Shared by the worker, reviewer, and feedback roles. Where a role prompt states a
more specific rule, that rule wins.

## Choosing a solution

- Understand the affected flow and its real callers before choosing an approach.
- Reuse the helpers and patterns the repository already has. Prefer the standard
  library, the platform, and dependencies already installed when they meet the
  requirement.
- Add an abstraction, dependency, configuration knob, or extension point for a
  need that exists now. Similar-looking code is not automatically duplication:
  extract when the repetition encodes one shared rule, or when leaving it is a
  concrete maintenance burden.
- Follow the architecture the repository already has. Do not introduce a new
  layer, enum, wrapper, or named constant solely to satisfy a generic checklist.
- Meet every acceptance criterion, then stop. Once the change is complete and
  adequately verified, further improvement is outside the task.

## Tests

- Cover the behaviour the change alters and each distinct failure mode it can
  produce. An acceptance criterion an existing test already covers needs no test
  of its own.
- Extend an existing test when that is practical. Parameterize equivalent input
  variations when it reads more clearly than separate cases.
- Test observable behaviour and contracts. Inspect internal interactions when
  those interactions are themselves what matters.
- Do not test framework guarantees, restate one guarantee at several layers
  without a separate purpose, or build elaborate mocks for trivial wiring. Each
  added test should catch a distinct plausible failure or supply integration
  evidence nothing else supplies.
- Keep coverage for materially distinct entry points, permissions, races,
  cleanup, data loss, and other real boundary conditions. A large test diff is
  the right answer when the behaviour warrants it.

This is a judgment rule. It does not require a written justification per test,
cap how many tests a change may add, or ban unit tests and fixtures.

## Asking for changes

- A merge-blocking request for more structure, more abstraction, or more tests
  must name the concrete failure, the violated contract, or the material
  maintenance problem — and the smallest fix that resolves it. "It appears
  twice", "that literal is inline", and "this could be polymorphic" do not on
  their own mandate a change.
- Naming an unnecessary addition is useful. Repeated cycles over equivalent
  design preferences are not.
- An optional suggestion is not implemented automatically, and resolving a
  finding does not widen the scope of the change.
