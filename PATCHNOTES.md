# Tokenade — what's new
## 2.0.3
- After a successful `tokenade install` (or, if that run was not in a terminal, the first `tokenade dashboard` with saved tokens to show), one line says Tokenade is open source and links the GitHub repository. Shown once per machine, never when stdout is not a terminal or under CI, and `TOKENADE_NO_STAR_INVITE=1` turns it off. It makes no network call.

## 2.0.2
- `tokenade gain` no longer prints a `metered:` line (and `gain --json` no longer has `operations_as_metered` / `saved_as_metered`): they described the metering of the former paid service.
- `tokenade read` with a relative path from a subdirectory of a git repository now recognises the repository, so a source file you are about to edit comes back unchanged instead of with its comments stripped.
- `tokenade read --help` now describes what happens to source code: unchanged when it is likely to be edited (inside a git repository, under ~20k tokens), comments stripped for other source files over 3 KB, `TOKENADE_NO_CODE_STRIP=1` to turn the stripping off.
- `tokenade expand-fold --help` shows its own help instead of the `expand-ref` one, and `expand-fold <template_id> --max N` no longer reads `N` as the content hash.
- Help texts and the one-time open-source notice trimmed.

## 2.0.1
- The 1.x → 2.0 cleanup now also removes `last-report.zip` (the support bundle of the removed `tokenade report`, which held a copy of agent transcripts) and a stray `reporter.lock`, including on machines already migrated to 2.0.0.


## 2.0.0
- **Tokenade is open source** (MIT OR Apache-2.0). No account, no login, no licence key, no trial, no quota: every feature works out of the box.
- **No telemetry.** Nothing is sent anywhere; your savings ledger and dashboard stay on your machine. The only network calls left are the ones you ask for (web fetch/search) and the daily signed update check, which now reads GitHub Releases (`TOKENADE_NO_AUTO_UPDATE=1` turns it off).
- `tokenade login`, `activate` and `report` are gone. On first run, leftover licence and usage-report files under `~/.tokenade` are removed once; your savings history is kept.


## Earlier releases
Release notes for 1.x and before: https://tokenade.net/en/docs/changelog
