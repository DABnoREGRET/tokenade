#!/usr/bin/env bash
# Clone the 12 benchmark target repos at PINNED upstream SHAs.
#
# Why pin: benchmark numbers (savings %, recall, F1) must be reproducible
# across machines and over time. A shallow `git clone` of `main` works
# today and breaks next week — the upstream repo moves on. The SHAs below
# were captured 2026-05-28 and represent the snapshot the v2 results in
# `benchmarks/results.json` were computed against.
#
# To refresh: re-run with new SHAs in the table below, then re-run
# `sts-bench` and commit both `clone_bench.sh` and `results.json` together.
#
# Behaviour: idempotent. Skips repos already at the pinned SHA. Checks out
# the pin if the clone is on a different commit. Re-clones if missing.

set +e

# `repo => "git_url|pinned_sha"`. SHAs captured 2026-05-28.
declare -A repos=(
  [anyhow]="https://github.com/dtolnay/anyhow|841522b2aa09732fecee40804440d2c35c68c480"
  [byteorder]="https://github.com/BurntSushi/byteorder|5a82625fae462e8ba64cec8146b24a372b4d75c6"
  [chalk]="https://github.com/chalk/chalk|aa06bb5ac3f14df9fda8cfb54274dfc165ddfdef"
  [click]="https://github.com/pallets/click|6a141c3681027e8124ce5a3c70e608dbbebffafb"
  [cobra]="https://github.com/spf13/cobra|ad460ea8f249db69c943a365fb84f3a59042d54e"
  [flask-tiny]="https://github.com/pallets/itsdangerous|672971d66a2ef9f85151e53283113f33d642dabd"
  [gin]="https://github.com/gin-gonic/gin|5f4f9643258dc2a65e684b63f12c8d543c936c67"
  [lerna]="https://github.com/lerna/lerna|f4387d673bfdf4923ab62cd52d3498dec6dc7f2c"
  [pydantic-core]="https://github.com/pydantic/pydantic|7870112d4e41405aaaf348e5e94a772fe512f26c"
  [ruff]="https://github.com/astral-sh/ruff|7e1bc1e75f15795f12c846294b13df4535f2abbf"
  [slugify]="https://github.com/sindresorhus/slugify|7c318bd1aa4b4affab29761f15a9604323fe2a3b"
  [zod]="https://github.com/colinhacks/zod|bbc68f990c7e6a5e3f506c56fb04bd0279b9c9b5"
)

for name in "${!repos[@]}"; do
  entry="${repos[$name]}"
  url="${entry%%|*}"
  pin="${entry##*|}"

  if [ -d "$name/.git" ]; then
    current=$(cd "$name" && git rev-parse HEAD 2>/dev/null)
    if [ "$current" = "$pin" ]; then
      echo "[pin-ok] $name @ $pin"
      continue
    fi
    # Repo present but at a different commit: try to align without re-clone.
    echo "[align]  $name → $pin"
    (cd "$name" && git fetch --depth 1 origin "$pin" >/dev/null 2>&1 && git checkout -q "$pin") \
      || echo "[FAIL]  $name: could not align to $pin"
    continue
  fi

  # No .git yet — fresh shallow clone of the pinned commit.
  echo "[clone] $name @ $pin"
  if timeout 120 git clone --depth 1 "$url" "$name" >/dev/null 2>&1; then
    (cd "$name" && git fetch --depth 1 origin "$pin" >/dev/null 2>&1 && git checkout -q "$pin") \
      || echo "[WARN] $name cloned but couldn't pin to $pin (network or shallow-history limitation)"
  else
    echo "[FAIL] $name: clone of $url failed"
  fi
done

echo "=== BENCH CLONE DONE ==="
du -sh */ 2>/dev/null | sort -h
