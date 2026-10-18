# Real accessibility snapshots

These are **captured**, not written. The a11y compactor used to be proved
against a React-shaped sample it invented itself (>25% asserted, and passing)
while real user traffic gave ~7% — the sample and the reality did not share a
shape. Anything measured here is measured on what a browser actually emits.

Format: the exact output of the Playwright MCP server's `browser_snapshot`
— refs, `[cursor=pointer]`, `[active]`, `/url:` lines and all.

Capture recipe (Playwright 1.61, chromium):

```js
const { chromium } = require('playwright');
const b = await chromium.launch();
const p = await (await b.newContext()).newPage();
await p.goto(url, { waitUntil: 'domcontentloaded' });
await p.waitForTimeout(2500);
const snapshot = await p.locator('body').ariaSnapshot({ mode: 'ai' });
```

`{ mode: 'ai' }` is the part that matters: without it you get an aria snapshot
with no refs, which is a different shape and compacts differently.

Any `*.a11y.txt` dropped in this directory is picked up automatically by
`tests/a11y_corpus.rs`. Prefer pages an agent would really drive: a code host, a
documentation page, a table-heavy legacy page, a long article.

Measured at the real cl100k tokenizer (`--features bpe`), not the heuristic
estimator — on structured input the estimator reports noticeably more saved
than a tokenizer does.
