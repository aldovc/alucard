# Demo plan — basic fixture

Shared context for the parser tests: constraints live here, above the first
task heading, and travel with every task as the parent context.

## [ ] 1: First queued task

What to build: the first eligible thing.

Blocked by: none

## [x] 2: Already done

This one is complete.

## [ ] 3: Blocked by an open task

Body text.

Blocked by: 1

## [ ] 4: Unblocked because its blocker is done

Blocked by: 2

## [>] 5: In flight (PR #123)

Has an open PR.

## [h] 6: Human task

Never queued.

## [ ] 7: Multi-blocker with one still open

Blocked by: 2, 3
