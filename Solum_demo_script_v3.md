# Solum demo — English narration (v3, 2026-07-21)

Honesty constraints applied: no claim that the runtime model is GPT-5.6 (config points at an
OpenAI-compatible gateway; the model name is never asserted). Codex limited to Daily Focus Brief.
GPT-5.6 credited as the coding agent, per the user's own account of who wrote the post-07-19 work.

---

**1. Opening (~0:00–0:16)**

"This is Solum — a personal agent that actually knows you. Rust core, Tauri shell,
desktop and Android. Every piece of personal data you are about to see lives in a
local SQLite database on this machine."

**2. Scheduling (~0:16–0:44)**

"I type one ordinary sentence. Solum parses it offline into a structured event, grades
how much it matters, and sets the reminder to match — a deadline gets a full day of lead
time, an exam gets three days, an ordinary meeting gets thirty minutes. That grading is
deterministic and needs no network at all."

**3. Agenda (~0:44–1:00)**

"Everything lands on one timeline, tagged by the kind of commitment it is."

**4. Notification capture (~1:00–1:24)**

"Solum also captures notifications from apps you have whitelisted — natively on Android.
A flight-change notice came in, and Solum found schedulable content inside it. But read
the last line: this app was never granted permission to write to your calendar, so Solum
stops and asks. Being allowed to read you is not the same as being allowed to act for
you. Those are two separate grants."

**5. Generative UI and custom components (~1:24–2:12)**

"Solum's replies are not just text. I ask for an expense tracker in plain language, and
the cloud model — reached through an OpenAI-compatible gateway — composes a declarative
schema: typed fields, and the views it should render as. The model proposes; it never
executes. Nothing is stored until I confirm. Now it is a real local component I can add
records to, and it syncs across my devices end-to-end encrypted."

**6. Memory ledger (~2:12–2:34)**

"Everything Solum remembers sits in one ledger — every event, reminder and suggestion
traced back to the raw sentence it came from, each one deletable on its own. Privacy
here is a feature, not a promise."

**7. Close (~2:34–2:56)**

"Solum is designed by me and built with AI coding agents. The Daily Focus Brief you saw
at the start was built end to end with OpenAI Codex. Most of the recent work — the
security-audit remediation, the persistent custom components, the email connector — was
implemented by GPT-5.6 against my design. And to be clear: Solum is not finished. It is
still under active development, and several parts you have just seen are early.
Thanks for watching."

---

## Claim check

| Claim | Basis |
|---|---|
| Rust core, Tauri desktop + Android | README repo structure |
| local SQLite only | ARCHITECTURE §4 privacy invariant |
| graded lead times 1d / 3d / 30m | observed live in this demo's seeded data |
| offline deterministic grading | F16 offline-first; classify.rs |
| notification capture native on Android | solum-notif-access plugin; desktop is no-op |
| capture grant ≠ auto-event grant | CHANGELOG 2026-07-21 Security, `auto_event_packages` |
| model proposes, never executes | genui.rs action whitelist |
| nothing stored until confirm | observed: preview → 确认创建 |
| widget defs + records sync | README F19 "定义与记录随 F17 同步" |
| ledger traceable + per-row delete | observed in 记忆台账 |
| Daily Focus Brief built with Codex | MISC.md 2026-07-15 / 2026-07-17 |
| GPT-5.6 implemented recent work | user's statement, 2026-07-21 |
| still under development | user's explicit instruction |
