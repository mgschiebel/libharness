<!-- SPDX-License-Identifier: MIT -->
<!-- Copyright (c) 2026 Michael Schiebel -->

# Wayfinder rounds: lessons learned

An experience report on a modified version of the wayfinder workflow, written to be read, taught from, and sent on. The rules for agents are in the companion file `for-agents.md`. This file explains why those rules exist.

## Origin and attribution

Everything here builds on the skills in [mattpocock/skills](https://github.com/mattpocock/skills) (MIT licence), especially `wayfinder`. The upstream skills `research`, `prototype`, `grilling` and `domain-modeling` were used unchanged. Only `wayfinder` was modified, by roughly forty lines. This file describes those changes in prose and does not reproduce upstream text.

What changed, in short:
- A new HITL ticket type, **recommend**, sits between the prototypes and grilling.
- Work on a topic runs as a **round**: research, prototypes, recommend, grilling.
- Research now ends by planning the round's prototypes.
- Prototypes became AFK and build-only. Upstream had them HITL.
- A parking mechanism lets AFK work ask a human question without blocking.
- Grilling starts from the recommendation, and a human-only skip exists for clear-cut topics.

## How to read the evidence

Every claim carries a tag.

- **Observed** means it happened in our rounds. Numbers are kept as they were, but the domain is stripped, so you will not find our project's subject here.
- **Hypothesis** means we designed for it, but have not seen it work or fail.

The sample is small: one human, one effort, about five topics, seven finished prototypes. At the time of writing, topics have completed research, prototypes and recommend, but no topic has yet finished the grilling stage. Everything about grilling's role in a round is therefore a hypothesis. Nothing here is proven.

## Why rounds exist

Wayfinder tickets are decisions, not build tasks. In upstream, the prototype ticket mixed two jobs: building a rough artifact and discussing it with the human. For us, that made prototype sessions long and unpredictable, and they ended with the discussion unrecorded or half-recorded. A topic also had no clear end. Grilling had to rediscover the evidence every time.

Rounds split the work so each ticket does one job. Research gathers facts. Prototypes test the candidates. Recommend is where the human reacts to evidence. Grilling decides at the wider scope. The rest of this file is the eight insights that fall out of that split.

## The eight insights

### 1. One job per ticket, in a fixed order

- **Problem:** tickets that research, build and discuss at once are hard to size and hard to resume.
- **Change:** the order is research, prototypes, recommend, grilling, and each stage stops at its own job.
- **Evidence:** `[observed]` The round order held for every topic we ran. `[hypothesis]` It makes sessions more predictable than the upstream layout, but we never measured that.

### 2. Research plans the round

- **Problem:** you cannot know how many prototypes a topic needs before the facts are in. Creating them at charting time produces tickets that get deleted or rewritten.
- **Change:** a research ticket's last act is to create the round's prototypes (one per leading candidate per genuine uncertainty), one recommend ticket and the topic's grilling ticket, and to wire them.
- **Evidence:** `[observed]` One research ticket created two prototypes, one recommend ticket and one grilling ticket, and the count of two was not knowable when the map was charted.

### 3. Prototypes build and only build

- **Problem:** a prototype session that also holds the review conversation tangles two jobs, and the human's reaction is lost when the build session ends.
- **Change:** prototypes are AFK and write a report. They never decide the topic. If they need a human answer, they park.
- **Evidence:** `[observed]` Five prototypes finished under this model. Each ended with a report that held its verdict for the recommend ticket. `[hypothesis]` Parking works. In our rounds, no ticket ever needed to park, so that mechanism is untested.

### 4. Recommend is the human's interface to the round

- **Problem:** after several prototypes, the human needs one place to answer questions and weigh the evidence together.
- **Change:** one ticket with two modes. Triage mode is open whenever something in the round is parked. Decision mode opens when all prototypes are closed. The agent recommends and the human decides.
- **Evidence:** `[observed]` Decision mode ran once. The human accepted the recommendation and added a constraint no prototype could have produced: a rule about not copying code from reference projects. `[hypothesis]` Triage mode works as designed. It has not run.

### 5. Grilling starts from the recommendation

- **Problem:** grilling that begins from nothing re-asks everything the round already settled.
- **Change:** grilling starts from the recommendation, can overturn it, and does not re-ask settled points. It is also the one place that writes decisions to the map.
- **Evidence:** `[hypothesis]` No topic has completed this stage yet. The structure is in place and one topic has reached it.

### 6. Only the human can skip grilling

- **Problem:** an agent that decides a topic is "clear-cut" will often be wrong, and will be most wrong when it is most confident.
- **Change:** grilling is the default. The agent never raises the skip. The human may invoke it only if every prototype reported no surprises and nothing touches another topic.
- **Evidence:** `[observed]` At least five of seven finished prototypes reported something unexpected, so a clear-cut prior would usually have been wrong. In the one decision session so far, the human chose not to skip. `[hypothesis]` The rule costs little and prevents mistakes, but nobody has invoked the skip yet.

### 7. Every prototype reports its surprises

- **Problem:** prototypes that report only "it worked" hide the facts that change decisions.
- **Change:** each prototype's report has a required **Surprises** section, and "none" is a valid entry. Recommend lists every surprise in its resolution comment.
- **Evidence:** `[observed]` Four surprises that would not have come out otherwise:
  - An integration layer estimated at 35 to 90 lines came to about 300.
  - A layered check against a class of forbidden behaviour still missed 9 of 29 attempts to get around it.
  - A build option for the host silently changed the feature set of a shared dependency.
  - A core component needed a 9-line patch to express one behaviour, and without it 6 of 9 scenarios failed.

### 8. Decisions rest on primary sources

- **Problem:** secondary summaries repeat each other's errors, and a decision resting on them cannot be checked.
- **Change:** every research claim links the primary source that owns it. Where no primary source exists, the claim is labelled "inference".
- **Evidence:** `[observed]` Research marked one mapping between two concepts as inference because no primary source covered it. Reading source code directly also showed that three candidate libraries failed a dependency policy check. Where findings are stored and how they reach everyone is left to each team. The requirement is that they are durable and linkable.

## A worked round (fictional)

This effort is invented. Its numbers are made up, to show the shape of a round and not to claim results.

**Map destination:** a written spec for how a note-taking app syncs edits made offline.

**1. Research ticket.** The question is "which sync approaches suit the app's constraints?" The agent reads protocol documentation and library source, then writes findings with a link per claim. It finds two leading candidates: per-field last-write-wins, and a text-merge data type. It notes one claim as inference: no documentation says how either behaves with attachments. Its last act creates:
- Prototype A: per-field last-write-wins.
- Prototype B: text-merge data type.
- One recommend ticket, blocked by A and B.
- The grilling ticket "Sync design", blocked by the recommend ticket.

**2. Prototype briefs.**

```markdown
## Question
Does per-field last-write-wins keep edits when two devices change one note offline?

## Validated means
Of 20 scripted conflicts, none loses an edit the user would consider theirs.

## Measure
Edits lost per 20 conflicts; code size (estimate first, then actual).
```

**3. Prototype B parks.** Midway, it needs to know whether a deleted note can be recovered after sync. It saves its work, comments the question, adds `awaiting-answer`, and stops.

**4. Recommend, triage mode.** The human runs the recommend ticket. The agent asks about the deleted-note question, the human says recovery must work for 30 days, and the agent posts the answer on Prototype B's ticket and resumes it.

**5. Prototypes finish.** Each report has Surprises:
- A: with a two-minute clock skew, 3 of 20 conflicts lost an edit.
- B: storage grew to about six times the plain-text size after 1,000 edits.

**6. Recommend, decision mode.** The agent proposes text-merge only for note bodies and last-write-wins for metadata. The human agrees. The resolution lists both surprises. The B surprise touches attachments, which is another topic, so the clear-cut skip is not allowed even if the human wanted it.

**7. Grilling.** It starts from the recommendation, settles open details, writes the decision to the map, and graduates a new fog item ("attachment sync") into its own ticket.

## Adopting this

1. Start with research feeding prototypes, plus recommend in decision mode. That is the part we have seen work.
2. Add the Surprises section to your prototype template on day one. It costs a few lines and paid off the most.
3. Add parking and triage mode when a prototype first needs a human mid-build. We have not yet needed it.
4. Keep the clear-cut skip human-only. Do not let an agent offer it.
5. Decide where your team stores findings and prototype code. These rules do not constrain that.
6. Tag your own claims observed or hypothesis for the first few rounds, so you know which parts of the process you have verified.

The cost: more tickets per topic, and one more human step (recommend) before the wider-scope decision.

## What we don't know

- Whether parking and triage mode work. They never ran.
- Whether grilling after recommend is shorter than grilling from scratch. No topic has finished that stage.
- Whether rounds save time overall. We did not measure effort without them.
- Whether any of this holds with more than one human, or on an effort that isn't a technical design.
- Whether the human-only skip will ever be used, or just adds ceremony.

## What we'd suggest upstream

These are suggestions to a project we borrowed from, not claims.

1. `[observed]` Add a required **Surprises** section to the prototype report. It surfaced the facts that changed the most decisions.
2. `[hypothesis]` Make prototype AFK build-only and add a recommend ticket as the human's interface. It separates two jobs that upstream mixes.
3. `[observed]` Let research end by planning the round, since the count of prototypes isn't knowable earlier.
4. `[hypothesis]` Keep any "clear-cut" skip under human control only.
5. `[observed]` Require primary-source links per research claim, with "inference" as an explicit label.

Happy to compare notes, share the anonymised ticket history behind the numbers, or help turn any of this into a smaller change to the upstream skill.
