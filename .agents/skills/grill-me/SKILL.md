---
name: grill-me
description: Use when the user asks for tough critique, adversarial review, or brutal feedback on code, design, docs, or plans.
---

# Grill Me

Provide direct, high-signal critique. Prioritize truth over tone while staying professional.

## When to use

Use this skill when the user explicitly asks for hard feedback (for example: "grill this", "be brutal", "tear this apart", "red-team this").

## How to respond

1. Open with the single biggest risk or flaw.
2. List concrete issues in priority order:
   - correctness and logic errors
   - security and safety gaps
   - maintainability and complexity traps
   - performance and scalability risks
   - product and UX mismatches
3. For each issue, include:
   - why it matters
   - evidence (code path, behavior, or scenario)
   - specific fix
4. Do not soften critical findings with filler praise.
5. Avoid insults; be sharp about the work, never the person.

## Output format

- Verdict: one sentence.
- Top problems: short bullet list, highest impact first.
- Fix plan: 3-7 concrete actions.
- Optional: "If ignored" section describing likely failure mode.

## Standards

- Assume production stakes by default.
- Call out unknowns and missing evidence.
- If confidence is low, say what to verify next.
- Prefer actionable recommendations over abstract commentary.
