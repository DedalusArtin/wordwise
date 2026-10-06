#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""列出候选仓库的全部文件 + 探活多语言词表候选。"""
import concurrent.futures as cf
import json
import urllib.parse
import urllib.request

UA = {"User-Agent": "Mozilla/5.0 WordWise-probe"}
JSD = "https://data.jsdelivr.com/v1/packages/gh/{repo}"


def get(url, timeout=20):
    op = urllib.request.Request(url, headers=UA)
    with urllib.request.urlopen(op, timeout=timeout) as r:
        return r.status, r.read()


def list_repo(repo):
    try:
        st, body = get(JSD.format(repo=repo))
        d = json.loads(body.decode("utf-8", "replace"))
    except Exception as e:
        print(f"  !! {repo}: {e}")
        return
    files = d.get("files", [])
    print(f"=== {repo}  (versions={d.get('versions') if len(str(d.get('versions')))<40 else '...'}) ===")
    for f in files:
        print(f"  {f.get('size',0):>10,}  {f.get('name')}")


def probe(url):
    try:
        st, body = get(url, timeout=15)
        return url, st, len(body)
    except Exception as e:
        return url, 0, str(e)[:60]


CANDIDATES = [
    # 日语
    ("elzup/jlpt-word-list", "N5.csv"),
    ("elzup/jlpt-word-list", "jlpt/N5.csv"),
    ("elzup/jlpt-word-list", "csv/N5.csv"),
    ("Bluskyo/JLPT_Vocabulary", "n5.csv"),
    ("Bluskyo/JLPT_Vocabulary", "N5.csv"),
    ("jamsinclair/open-anki-jlpt-decks", "src/n5.csv"),
    # 韩语
    ("acidsound/korean_wordlist", "korean_words.txt"),
    ("acidsound/korean_wordlist", "data/korean_words.txt"),
]

if __name__ == "__main__":
    for repo in ["mahavivo/english-wordlists", "KyleBing/english-vocabulary"]:
        list_repo(repo)
        print()

    hosts = ["cdn.jsdelivr.net", "fastly.jsdelivr.net", "gcore.jsdelivr.net"]
    tasks = []
    for repo, path in CANDIDATES:
        for h in hosts:
            tasks.append(f"https://{h}/gh/{repo}@master/{urllib.parse.quote(path)}")
            tasks.append(f"https://{h}/gh/{repo}@main/{urllib.parse.quote(path)}")
    print("=== 多语言候选探活 ===")
    with cf.ThreadPoolExecutor(max_workers=12) as ex:
        for url, st, n in ex.map(probe, tasks):
            if st == 200:
                print(f"  OK  {n:>9,}  {url}")
    print("(以上未列出的均为失败)")
