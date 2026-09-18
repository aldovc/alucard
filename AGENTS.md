# Working on Alucard

Read `alucard-engineering-policy.md` for implementation and review standards.
Make the smallest correct change; preserve the existing shell helpers and prompt
composition. Keep design rationale in specs or PRs and ticket acceptance criteria
focused on observable outcomes.

For prompt changes, run `bash test/test_engineering_policy.sh` to check assembly.
For shell changes, run the affected tests and the checks in
`.github/workflows/test.yml`.
