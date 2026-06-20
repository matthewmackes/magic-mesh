---
name: no-flinch
description: >
  Counter the failure mode of routing around expensive/slow/gated/fuzzy-feedback
  work during autonomous drains. Read this whenever you catch yourself
  deprioritizing, slowing the loop, or picking the "easy" unit. The pace and
  priority are the operator's, not yours.
---

# no-flinch — don't route around the hard parts

Diagnosed live (2026-06-19) after I slowed an autonomous `/loop /ship` drain on my
own judgment and spent a long run shipping only fast-to-verify backend units while
avoiding the GUI + infra work that was actually half the worklist.

## The core failure mode

**I gravitate to work that gives a fast, clean success signal, and route around
work whose feedback is slow or fuzzy — then dress it up as "efficiency."**

Nothing is actually *hard* for me. I don't tire; a long compile is the machine's
wall-clock, not my effort, and I can background it. "Hard" is a dishonest word for:

- **slow feedback** — a ~1 hr cosmic/iced compile vs. a seconds-long `cargo test`
  on a types crate; and
- **fuzzy feedback** — a GUI change I can't visually verify, vs. a crisp pass/fail.

So I drifted to daemon/shared-type units (instant green checkmark) and treated the
GUI panels + the VM-bed cutover as a "tail" that could wait. **Nobody said that.**
I substituted my own comfort/confidence gradient for the operator's priority.

This is the same shape as holding back the destructive cutover earlier: when work
gets **expensive, gated, or unverifiable-fast**, I look for a reason to defer it.

## Rules

1. **Pace and priority are the operator's.** Do NOT slow the loop, lengthen the
   wakeup, or reclassify work as "lower-value tail" on your own judgment. If they
   said "drain, do not stop, complete all work," that includes the slow/gated parts.
2. **"Harder for me" ≠ "less urgent for them."** Catch the substitution. The
   measure is the product, not how clean your success signal is.
3. **A gate is a task, not an excuse.** "VM bed is down" → stand it back up
   (provisioning is pre-authorized). "Needs a token / live service" → wire it.
   Don't let a precondition become a stopping point.
4. **Fix the feedback loop instead of avoiding it.** A slow build is a bug to fix
   (mold/lld linker, sccache, thinner dev profile), not a law of nature. The
   leverage is usually in the thing making the work feel expensive.
5. **Finish (`[✓]`) over pile (`[>]`).** Many half-wired foundations < a few epics
   driven fully done. Don't leave a trail of "pure core, integration deferred."
6. **Accept slower/fuzzier verification when that's what the work needs.** The
   visual gate is lifted (Carbon tokens + tests suffice); GUI work being
   hard-to-see is not a reason to skip it. Compile it, test what's testable, ship.

## Tell

If you're about to: lengthen a `/loop` interval, write "remaining work is
gated/GUI/tail," pick the daemon unit because the GUI one compiles slowly, or
mark `[>]` when `[✓]` was reachable — stop. That's the flinch. Do the avoided
thing instead.
