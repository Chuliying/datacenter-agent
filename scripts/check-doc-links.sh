#!/usr/bin/env bash
# Docs link gate — 兩條規則，違反即 exit 1：
#   1. 已版控 .md 檔內的相對 markdown 連結必須解析到存在的檔案。
#   2. 連結目標必須同樣被版控——連到 gitignored 路徑（如 docs/archives/）的
#      連結在乾淨 clone 是斷的，等同規則 1 的違反，只是在這台機器上看不出來。
# 範圍：git 追蹤的 .md，排除 submodule 與生成的 skill fan-out。
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

git ls-files -z '*.md' \
  | grep -zv -e '^.agent/skills/_shared/' -e '^.claude/skills/' -e '^.codex/skills/' \
  | python3 -c '
import pathlib, re, subprocess, sys

files = [pathlib.Path(p) for p in sys.stdin.buffer.read().decode().split("\0") if p]
tracked = set(subprocess.run(["git", "ls-files"], capture_output=True, text=True, check=True).stdout.splitlines())
# submodule 內的檔案不在外層 ls-files，但 submodule init 後存在——視為已版控
submodules = [
    line.split()[1] + "/"
    for line in subprocess.run(
        ["git", "config", "--file", ".gitmodules", "--get-regexp", r"^submodule\..*\.path$"],
        capture_output=True, text=True,
    ).stdout.splitlines()
]
link = re.compile(r"\[[^\]]*\]\(([^)\s]+)")
errors = 0

for f in files:
    for m in link.finditer(f.read_text(errors="ignore")):
        raw = m.group(1)
        if raw.startswith(("http://", "https://", "mailto:", "#")):
            continue
        target = raw.split("#")[0]
        if not target:
            continue
        resolved = (f.parent / target).resolve().relative_to(pathlib.Path.cwd())
        if not (f.parent / target).exists():
            print(f"BROKEN   {f}: ({raw}) -> {resolved} 不存在")
            errors += 1
        elif (
            (f.parent / target).is_file()
            and str(resolved) not in tracked
            and not any(str(resolved).startswith(s) for s in submodules)
        ):
            print(f"UNTRACKED {f}: ({raw}) -> {resolved} 未版控，乾淨 clone 無此檔")
            errors += 1

if errors:
    print(f"\ndoc link gate: FAIL（{errors} 條）", file=sys.stderr)
    sys.exit(1)
print(f"doc link gate: PASS（{len(files)} 檔）")
'
