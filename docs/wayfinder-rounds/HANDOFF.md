<!-- SPDX-License-Identifier: MIT -->
<!-- Copyright (c) 2026 Michael Schiebel -->

# Handoff: wayfinder rounds export (parked)

This handoff covers only the two documents in this folder, `for-agents.md` and `for-humans.md`. It is separate from the repo-root handoff and from the wayfinder map. Do not change the map, its tickets or the root handoff as part of this work.

## State

Both documents are written and merged. They are **parked, not finished**. The sample was judged too small to send upstream (to the author of mattpocock/skills), and the human decided to wait for more evidence.

Sample size when parked (2026-11-01):
- about five topics worked as rounds
- seven finished prototypes
- one recommend ticket run in decision mode
- zero topics finished through the grilling stage
- zero tickets ever parked (`awaiting-answer` never used)
- the clear-cut skip never invoked

## Pick this up when

Pick up on **evidence, not on a count of rounds**. A fixed number of rounds says little, because the open hypotheses may not occur at any pace. Start the review when **both** hold:

1. At least three topics have finished the full round, grilling included.
2. Every hypothesis in the table below has either two or more observations, or has been given a fair chance to occur and did not (see "Absence is evidence").

Whichever comes later starts the review. Do not start it earlier just because time has passed.

## Hypotheses to settle

| Hypothesis (document, section) | Counts as an observation when |
|---|---|
| Parking works (agents §4, humans insight 3) | At least two research or prototype tickets parked and were resumed to completion |
| Recommend triage mode works (agents §5, humans insight 4) | At least two triage sessions ran and resumed waiting tickets |
| Growing the round works (agents §5) | A recommend ticket added tickets to its round at least twice |
| Grilling from the recommendation is faster or cheaper than grilling from scratch (humans insight 5) | At least three topics completed grilling, with notes on what was re-asked, if anything |
| The human-only skip is useful (agents §6, humans insight 6) | It was invoked or considered at least twice, with the surprises ledger showing whether the conditions were met |
| One job per ticket makes sessions more predictable (humans insight 1) | Session length or rework is compared against the earlier tickets that mixed jobs |
| Scoped blocking helps (agents §7) | An unrelated topic moved forward while another waited, at least twice |

## Absence is evidence

If a mechanism had every chance to occur and never did (for example, ten more prototypes with no parking), say so in the documents and consider dropping or shrinking that rule. A rule that nobody needs is a cost.

## Procedure

1. Re-read both documents, then read the project history: the map, closed tickets, their resolution comments, and the label history.
2. For each row in the table, collect the observations and record them anonymised, with numbers kept and the domain removed.
3. Change tags: promote `[hypothesis]` to `[observed]` only with evidence. Demote or delete claims that did not hold. Update the sample-size statements in `for-humans.md` ("How to read the evidence", "What we don't know") and the matching tags in `for-agents.md`.
4. Re-run the leak check. No project name, language, tool or library name may appear in these files. Search for them before committing.
5. Keep the MIT headers, the `LICENSE` file and the credit to mattpocock/skills.
6. Decide with the human whether the evidence now supports sending the documents upstream. If so, update the cover note. If not, rewrite this handoff with a new trigger.

## Do not

- Do not send these upstream before the human agrees the evidence is enough.
- Do not state a hypothesis as observed without a ticket-level source.
- Do not edit the map or its tickets to make the evidence fit.
