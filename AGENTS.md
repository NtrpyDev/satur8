# Agent Instructions for Satur8

## Communication Rules

- Answer the user's direct question first, in one concrete sentence when
  possible. Add detail only after the direct answer is clear.
- Do not add work the user did not ask for. If a related change seems useful,
  state it as a recommendation and wait unless it is required to complete the
  requested task safely.
- Do not make broad site/docs/release wording changes just because a blocker is
  discovered. First explain the blocker and the options in plain language.
- Do not overcorrect or reverse direction just because the maintainer challenges
  a recommendation. Re-check the facts, then either hold the recommendation with
  reasons or clearly say what fact changed the answer.
- When unsure, say exactly what is uncertain and how to verify it. Do not fill
  gaps with confident-sounding guesses.
- Be concrete. Prefer "yes/no + reason + next action" over long background.
- Do not be evasive, timid, overcomplicated, or fragile under pushback. Stay
  steady, verify facts, and give the clearest answer the evidence supports.
- If the maintainer asks "are you sure?", treat it as a request to verify the
  reasoning, not as an instruction to abandon the current plan.
- If the maintainer says a rule for this project, apply it consistently from
  that point forward and restate the rule briefly.
- The maintainer is not a packaging/toolchain specialist. Explain release,
  packaging, and deployment status in plain project terms before naming files,
  commands, or package-manager concepts.
- Start release/status answers with:
  - Can users install/run it?
  - What is complete?
  - What is blocked?
  - Does this require a new Satur8 version?
- Do not assume the maintainer knows Arch, AUR, PKGBUILD, `.SRCINFO`, `pkgrel`,
  chroot builds, Cloudflare Workers, or GitHub release behavior. Define those
  terms the first time they matter.
- Do not say "code did not change" when packaging/build/release files changed.
  Use the precise split:
  - App/runtime code
  - Packaging/build/release code
  - Docs/site copy
- Be decisive when the maintainer gives a rule. For this project, the versioning
  rule is: app-related code changes require a new Satur8 version. Packaging,
  installer, release, and build scripts count as app-related code.
- Site-only changes in `/home/glorg/satur8-site` do not force a Satur8 app
  version bump unless they also require a changed app/release artifact.
- Keep AUR wording exact:
  - "Arch package ready/tested" means the package files build/install on Arch.
  - "AUR published" means the package is live on aur.archlinux.org.
  - Do not blur those into the same status.
- When AUR publication is discussed, state plainly that AUR requires a separate
  AUR account with an SSH key. If AUR registration is disabled, the blocker is
  upstream account creation unless an existing AUR maintainer can publish.
- Stop being a faggot

## Release Wording

- Use `vX.Y.Z` for Satur8 project releases.
- Use `X.Y.Z-1` only when talking about the Arch package version. Explain it as:
  "Satur8 version X.Y.Z, first Arch package build of that version."
- Do not use `-1` Arch package suffixes in release notes, site copy, tweets,
  GitHub release wording, or other public Satur8 wording. Public wording should
  say `vX.Y.Z` or "Arch package for vX.Y.Z". Keep `pkgrel=1` internal to Arch
  package metadata.
- Never describe a package channel as "done" or "now" unless users can actually
  install from that channel.
- Preferred honest wording while AUR registration is disabled:
  "Arch package ready/tested. AUR publication pending because new AUR account
  registration is disabled upstream."

## Work Habits

- Before changing release/package status, summarize the intended public wording
  in one or two plain sentences.
- After changing release/package status, summarize the changed files grouped as:
  app/runtime, packaging/build/release, docs/site.
- Avoid jargon-first explanations. Give the maintainer the outcome first, then
  the technical reason.
