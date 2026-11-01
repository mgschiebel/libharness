<!-- SPDX-License-Identifier: MIT -->
<!-- Copyright (c) 2026 Michael Schiebel -->

# Wayfinder rounds: rules for agents

Apply these rules when you work a wayfinder map (a planning map of decision tickets on an issue tracker) and the effort uses **rounds**. They extend the upstream `wayfinder` skill from [mattpocock/skills](https://github.com/mattpocock/skills) (MIT). Where a rule here conflicts with the upstream skill, this file wins for the effort. The upstream skills `research`, `prototype`, `grilling` and `domain-modeling` are used unchanged.

Terms: **map** is the index issue. **Ticket** is a child issue holding one decision or investigation. **AFK** means you work alone. **HITL** means you work with the human live, and you never answer for them. **Frontier** is the open, unblocked, unclaimed tickets. Tracker wording is generic: "label", "blocked by" and "comment" map to whatever your tracker offers. GitHub Issues is one example.

Confidence tags: `[observed]` means it happened in real rounds. `[hypothesis]` means it is a design intent not yet exercised or not yet seen to hold. Treat hypotheses as rules to follow, but expect to revise them.

## 1. The round

A topic is worked as a round, in this order: **research → prototypes → recommend → grilling**. Each ticket type does one job and nothing else.

- Research finds facts and decides what is worth prototyping.
- Prototypes build cheap artifacts that test the leading candidates.
- Recommend turns the evidence into a topic-level recommendation with the human.
- Grilling decides at the wider scope, across topics and fog.

`[observed]` A round run this way produced a surprise in almost every prototype round (see section 6).

## 2. Ticket types and labels

Each ticket carries exactly one type label. Never put a triage label (like "ready for agent") on a map ticket.

| Label | Mode | Job |
|---|---|---|
| `research` | AFK | Read primary sources, write findings, plan the round |
| `prototype` | AFK | Build only, to answer one question in its brief |
| `recommend` | HITL | Triage parked questions, then reach a mutual topic recommendation |
| `grilling` | HITL | Wider-scope decision, starting from the recommendation |
| `task` | either | Manual work that unblocks a decision (unchanged from upstream) |

One transient label, `awaiting-answer`, marks a research or prototype ticket parked on a human question.

## 3. Research tickets

1. Investigate against **primary sources** (official docs, specs, source, first-party APIs). Follow every claim to the source that owns it.
2. Write findings with a source link per claim. Where no primary source exists, say "inference" next to the claim. `[observed]`
3. Record findings durably, where every later agent session can read them and a ticket comment can point at them. How and where is the adopter's choice. This file does not prescribe a storage or branching strategy.
4. **Last act: plan the round.** Look at the leading results and create, as children of the map:
   - one **prototype** per leading candidate per genuine uncertainty. Skip any prototype that would not change a decision. Each gets a brief (section 10) and is blocked by the research ticket.
   - one **recommend** ticket, blocked by all those prototypes.
   - the topic's **grilling** ticket, blocked by the recommend ticket. Create it if it does not exist, otherwise add the edge.
5. If nothing is worth prototyping, create no recommend ticket and block the grilling ticket on the research ticket directly.
6. Do not pre-create prototypes before research closes. How many you need is fog until then. `[observed]`
7. Create tickets first, then wire blocking edges in a second pass, using real ids.

## 4. Prototype tickets

1. Claim the ticket (assign it) before any work.
2. Build the cheapest artifact that answers the brief's question. Throwaway, clearly marked, no polish.
3. **Build only.** Never hold a review conversation. Never make the topic decision. `[observed]` Prototypes that "held" their recommendation for the recommend ticket kept the decision with the human.
4. Report in the resolution comment: a short gist, measurements against the brief, and a **Surprises** section. If there were no surprises write "none". Never omit the section. The recommend ticket and the skip rule read it.
5. Report estimate versus actual wherever the brief asked for a measurement. `[observed]` Estimates for the cost of integrating two parts were off by a factor of three to nine.
6. Keep the prototype itself reachable as a primary source. Where it lives is the adopter's choice.

### Parking

`[hypothesis]` If you need a human answer to continue, do not wait. Save your work, post the question as a comment, add `awaiting-answer`, and stop. After the answer is posted, a fresh agent resumes from your saved work and the comments, and removes the label.

## 5. Recommend tickets

One ticket, two modes. The ticket stays natively blocked by its prototypes the whole time.

**Triage mode** `[hypothesis]`. Takeable whenever any ticket in its round has `awaiting-answer`. Claim it, grill the human on each parked question, post each answer as a comment on the waiting ticket, remove the label, resume the waiting work, release the claim. The ticket stays open. Triage resolves nothing and does not count as the one ticket you resolve in this session.

**Decision mode** `[observed]`. Takeable when every blocker is closed. Claim it, read every prototype output (including every Surprises section), draft a recommendation, and grill the human to a mutual recommendation scoped to the topic: which candidate, and why. Post it as the resolution comment and close. **You recommend, the human decides.**

The resolution comment must:
1. state the recommendation and its reasons, each reason linked to a finding or prototype result,
2. list every surprise the prototypes reported, or say "none",
3. say whether grilling runs next (default yes).

**Growing the round** `[hypothesis]`. In either mode you may create more research or prototype tickets in the round. Wire each as a blocker of the recommend ticket and comment why. The ticket returns to blocked. If the leading candidates fail, add a new batch the same way.

## 6. Grilling after recommend

1. Start from the recommendation. Do not re-ask what it settled. You may overturn it if the wider scope demands that.
2. Grilling is the writer to the map: it appends the decision to the map's decisions list and graduates any fog now specifiable.
3. Grilling is the default. `[observed]` In the rounds so far, at least five of seven finished prototypes reported something unexpected, so a "this is clear-cut" prior was wrong.

### The clear-cut skip

`[hypothesis]` Never raise the skip, and never ask whether a topic is clear-cut. Only the **human** may invoke it, during recommend's decision mode. It is allowed only when **both** hold:
1. every prototype in the round reported "none" under Surprises (or only confirmed expectations), and
2. nothing in the round touches another topic.

If either fails, grilling runs whatever anyone thinks. If the skip is invoked and allowed, the recommend session writes the map line itself, graduates any fog it cleared, and closes the grilling ticket with the comment "resolved by recommend".

## 7. Blocking

1. Use the tracker's native "blocked by" relationship where it exists. A ticket is unblocked when every blocker is closed.
2. A grilling ticket is blocked by its own topic's recommend ticket (or its research ticket if there are no prototypes). Topics that are unrelated do not block each other. `[observed]`

## 8. Session discipline

- Claim a ticket (assign it) before work.
- Resolve at most one ticket per session. Exceptions: research tickets may be resolved together, and recommend's triage mode resolves nothing.
- A HITL ticket resolves only through a live exchange. Never answer your own questions.

## 9. Do not

- Do not pre-create prototypes before research closes.
- Do not let a prototype decide the topic or run a review conversation.
- Do not omit the Surprises section, even when it is "none".
- Do not raise the clear-cut skip yourself.
- Do not state a hypothesis as proven when you summarise this process to others.

## 10. Templates

**Prototype brief**

```markdown
## Question
<the single uncertainty this prototype answers>

## Validated means
<observable conditions that would make the answer "yes">

## Measure
<numbers to record; include your estimate before building, then the actual>

## Report
Gist, measurements, Surprises (write "none" if none).
```

**Recommend ticket body**

```markdown
## Question
<topic>: which candidate, and why?

## Round
Research: <name>. Prototypes: <names>.
```

**Recommend resolution comment**

```markdown
## Recommendation
<one or two lines>

## Reasons
- <reason> (<link to finding or prototype result>)

## Surprises found
- <surprise> / none

## Next
Grilling runs. / Clear-cut skip invoked by the human (all prototypes "none", no cross-topic effects).
```

**Parking comment** `[hypothesis]`

```markdown
Parked: <the question, with the options you see>. Work saved at <where>. Awaiting an answer.
```
