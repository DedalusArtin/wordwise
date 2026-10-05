#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""用 GitHub API 列出仓库文件树（用于挑选可下载的词表文件）。"""
import json
import sys
import urllib.request

UA = {"User-Agent": "WordWise-probe", "Accept": "application/vnd.github+json"}


def api(url):
    op = urllib.request.Request(url, headers=UA)
    with urllib.request.urlopen(op, timeout=25) as r:
        return json.loads(r.read().decode("utf-8", "replace"))


def tree(repo):
    last = None
    for b in ("master", "main"):
        try:
            d = api(f"https://api.github.com/repos/{repo}/git/trees/{b}?recursive=1")
            return b, d.get("tree", [])
        except Exception as e:
            last = e
    raise last


def main():
    for repo in sys.argv[1:]:
        try:
            br, t = tree(repo)
        except Exception as e:
            print(f"=== {repo}: 失败 {e}\n")
            continue
        print(f"=== {repo} @ {br} ===")
        for f in t:
            if f.get("type") != "blob":
                continue
            name = f["path"]
            if name.lower().endswith((".txt", ".csv", ".tsv", ".json", ".md")):
                print(f"  {f.get('size',0):>10,}  {name}")
        try:
            lic = api(f"https://api.github.com/repos/{repo}/license")
            print(f"  LICENSE: {lic.get('license', {}).get('spdx_id')} / {lic.get('name')}")
        except Exception as e:
            print(f"  LICENSE: 读不到 ({e})")
        print()


if __name__ == "__main__":
    main()
