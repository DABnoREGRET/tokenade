# Security policy

## Reporting a vulnerability

Please **do not open a public issue** for security problems.

Report them privately through GitHub:
**Security → Advisories → Report a vulnerability** on
<https://github.com/pi-infected/tokenade/security/advisories/new>.

Include the affected version (`tokenade --version`), your OS and agent, and a
minimal reproduction. You should get a first answer within a few days. Once a
fix is released, the advisory is published with credit unless you prefer to
stay anonymous.

## Supported versions

Only the latest release receives security fixes. Tokenade updates itself
daily unless `TOKENADE_NO_AUTO_UPDATE=1` is set; otherwise update with
`tokenade upgrade` or `npm i -g @tokenade/cli@latest`.

## Scope

Of particular interest:

- the auto-updater and the npm installer (manifest signature and SHA-256
  checks, binary swap);
- secret redaction (a credential that survives into a fold, cache, log or
  the savings ledger);
- hooks, MCP proxy and shell integration (command injection, path traversal,
  a project file that makes Tokenade run or rewrite something it should not);
- the `execute` runner and repository-provided filters (`trust-filters`).
