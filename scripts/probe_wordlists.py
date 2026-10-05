#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""探活候选在线词库 URL。
用法:
    python scripts/probe_wordlists.py            # 探活内置候选清单
    python scripts/probe_wordlists.py -o out.json
"""
import concurrent.futures as cf
import json
import sys
import urllib.parse
import urllib.request

UA = {"User-Agent": "WordWise-probe/1.0"}

# jsDelivr 列出仓库文件（含子目录）
JSD_LIST = "https://data.jsdelivr.com/v1/packages/gh/{repo}?structure=flat"


def http(url, timeout=15, head=False):
    op = urllib.request.Request(url, headers=UA, method="HEAD" if head else "GET")
    with urllib.request.urlopen(op, timeout=timeout) as r:
        body = b"" if head else r.read(4096)
        return r.status, r.headers.get("Content-Length", ""), body


def list_repo(repo):
    """列出 jsDelivr 上某个 GitHub 仓库的全部文件。"""
    try:
        _, _, body = http(JSD_LIST.format(repo=repo))
        d = json.loads(body.decode("utf-8", "replace"))
    except Exception as e:
        return None, f"list failed: {e}"
    files = d.get("files", [])
    out = []
    for f in files:
        name = f.get("name", "")
        if name.lower().endswith((".txt", ".csv", ".json", ".tsv")):
            out.append((name, f.get("size", 0)))
    return out, None


def probe_one(item):
    """item = (label, url)"""
    label, url = item
    try:
        st, length, _ = http(url, head=True, timeout=12)
        return label, st, length, url, ""
    except Exception as e:
        return label, 0, "", url, type(e).__name__ + ": " + str(e)[:80]


def main():
    only_list = "--list" in sys.argv
    repos = sys.argv[1:] if len(sys.argv) > 1 else []

    if only_list:
        for repo in repos or ["mahavivo/english-wordlists"]:
            files, err = list_repo(repo)
            print(f"=== {repo} ===")
            if err:
                print("  ", err)
                continue
            for n, s in sorted(files):
                print(f"  {s:>10}  {n}")
        return

    # 候选清单：(标签, 仓库, 路径)
    CAND = []
    MAHA = "mahavivo/english-wordlists"
    for p in [
        "CET4_edited.txt", "CET6_edited.txt", "CET_4+6_edited.txt",
        "TOEFL.txt", "GRE_8000_Words.txt", "Highschool_edited.txt",
        "OALD8_abridged_edited.txt", "COCA_abridged.txt",
        "英语专业四八级词汇表.txt", "NPEE_Wordlist.txt",
        "COCA_20000.txt", "COCA_with_translation.txt",
        "GRE_abridged.txt", "TOEFL_abridged.txt",
        "中考英语词汇表.txt", "小学英语大纲词汇.txt",
        "红宝书 GRE词汇精选.csv", "英语专业星标八级词汇.txt",
        "英语六级词汇（星标，1726）.txt", "台灣高中英文參考詞彙表.txt",
        "IELTS.txt", "BEC.txt", "SAT.txt", "MBA.txt",
        "研究生英语词汇.txt", "大学四级词汇.txt",
    ]:
        CAND.append((f"MAHA/{p}", MAHA, p))

    KYLE = "KyleBing/english-vocabulary"
    for p in ["1 初中-乱序.txt", "2 高中-乱序.txt", "5 考研-乱序.txt",
              "3 四级-乱序.txt", "4 六级-乱序.txt", "6 雅思-乱序.txt",
              "7 托福-乱序.txt"]:
        CAND.append((f"KYLE/{p}", KYLE, p))

    # 多语言候选
    EXTRA = [
        ("JLPT/N5", "elzup/jlpt-word-list", "N5.csv"),
        ("JLPT/N4", "elzup/jlpt-word-list", "N4.csv"),
        ("JLPT/N3", "elzup/jlpt-word-list", "N3.csv"),
        ("JLPT/N2", "elzup/jlpt-word-list", "N2.csv"),
        ("JLPT/N1", "elzup/jlpt-word-list", "N1.csv"),
        ("JLPT/N5b", "watanabeyu/jlpt-vocabulary", "N5.csv"),
    ]
    for lbl, repo, p in EXTRA:
        CAND.append((lbl, repo, p))

    hosts = ["cdn.jsdelivr.net", "fastly.jsdelivr.net", "gcore.jsdelivr.net"]
    tasks = []
    for lbl, repo, p in CAND:
        enc = urllib.parse.quote(p)
        tasks.append((lbl, f"https://{hosts[0]}/gh/{repo}@master/{enc}"))

    print(f"probing {len(tasks)} urls...\n")
    res = []
    with cf.ThreadPoolExecutor(max_workers=12) as ex:
        for r in ex.map(probe_one, tasks):
            res.append(r)

    ok, bad = [], []
    for label, st, length, url, err in res:
        if st == 200:
            ok.append((label, int(length or 0), url))
        else:
            bad.append((label, st or err, url))

    print("=== OK (%d) ===" % len(ok))
    for label, size, url in sorted(ok):
        print(f"  {size:>10,}  {label}")
    print("\n=== FAIL (%d) ===" % len(bad))
    for label, st, url in sorted(bad):
        print(f"  {st!s:<28} {label}")

    out = next((a for a in sys.argv if a.startswith("-o=")), None)
    if out:
        with open(out[3:], "w", encoding="utf-8") as f:
            json.dump({"ok": ok, "fail": bad}, f, ensure_ascii=False, indent=2)
        print("\nwrote", out[3:])


if __name__ == "__main__":
    main()
