# Agents: what goes where

PodMesh lives in two repositories and one vault. Every agent working on PodMesh (Claude Code, Codex, Cursor,
Antigravity or any other) follows this split. Dialogue with the operator is French; everything written into either
repository is English.

| Where | What belongs there | What never goes there |
| --- | --- | --- |
| **`xavdp-pro/podmesh`** (this repository, public) | The product: code, tests, tools, packaging; the documentation needed to understand, build, qualify and operate PodMesh; the ideal scene, its program and its certification criteria (the ideal scene is common to both repositories and lives here only) | Laboratory status and campaign narratives, reviews, decisions and coordination notes, laboratory addresses and host names as defaults, anything private |
| **`xavdp-pro/podmesh-lab`** (private) | The maintainers' workshop: the current state and handoff, the status of every certification item with its evidence, campaign scripts and records, reviews, decisions, mandates, research notes, agent handoffs, laboratory configuration without secrets; it pins this repository as a submodule to read the ideal scene without copying it | Secrets and private kits, large binary evidence |
| **The vault** (or an encrypted backup the operator names) | Signing keys, tokens, credentials, private replica kits | — |

Rules:
1. A document that tells an adopter what PodMesh is, promises or how to operate it goes here. A document that tells the
   maintainers where the work stands, what was measured on their laboratory, or who decided what goes to `podmesh-lab`.
2. The ideal scene, the program and the certification criteria are edited here only. Their status (which item is met,
   with which evidence) is kept in `podmesh-lab/status/`.
3. Tools and tests take their hosts, addresses and workstation paths from the environment; no laboratory address, host
   name or workstation path is ever a default.
4. Nothing durable is kept only in a temporary directory.
