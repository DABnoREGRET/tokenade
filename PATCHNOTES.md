# Tokenade — what's new
## 2.0.1
- The 1.x → 2.0 cleanup now also removes `last-report.zip` (the support bundle of the removed `tokenade report`, which held a copy of agent transcripts) and a stray `reporter.lock`, including on machines already migrated to 2.0.0.


## 2.0.0
- **Tokenade is open source** (MIT OR Apache-2.0). No account, no login, no licence key, no trial, no quota: every feature works out of the box.
- **No telemetry.** Nothing is sent anywhere; your savings ledger and dashboard stay on your machine. The only network calls left are the ones you ask for (web fetch/search) and the daily signed update check, which now reads GitHub Releases (`TOKENADE_NO_AUTO_UPDATE=1` turns it off).
- `tokenade login`, `activate` and `report` are gone. On first run, leftover licence and usage-report files under `~/.tokenade` are removed once; your savings history is kept.


## Earlier releases
Release notes for 1.x and before: https://tokenade.net/en/docs/changelog
