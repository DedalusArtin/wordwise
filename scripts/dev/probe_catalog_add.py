#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""校验「待新增进目录」的候选词库文件是否真实可下载。

输出：状态 / 字节数 / 估算词条数 / 前两行样本（用来确认格式能被解析器识别）。
用法：python scripts/probe_catalog_add.py
"""
import concurrent.futures as cf
import ssl
import urllib.request

HOSTS = ["cdn.jsdelivr.net", "fastly.jsdelivr.net", "gcore.jsdelivr.net"]
UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64)"

# (标签, owner/repo@branch, 文件路径)
CANDS = [
    ("NPEE 考研",      "mahavivo/english-wordlists@master", "NPEE_Wordlist.txt"),
    ("COCA 2万",       "mahavivo/english-wordlists@master", "COCA_20000.txt"),
    ("COCA 带释义",    "mahavivo/english-wordlists@master", "COCA_with_translation.txt"),
    ("GRE 精简",       "mahavivo/english-wordlists@master", "GRE_abridged.txt"),
    ("GRE 去重",       "mahavivo/english-wordlists@master", "GRE_delete_CET4+6+TOEFL.txt"),
    ("四六托G 汇总",   "mahavivo/english-wordlists@master", "SUM_of_cet4+6+toefl+gre.txt"),
    ("托福精简",       "mahavivo/english-wordlists@master", "TOEFL_abridged.txt"),
    ("托福去重",       "mahavivo/english-wordlists@master", "TOEFL_delete_CET4+6.txt"),
    ("中考",           "mahavivo/english-wordlists@master", "中考英语词汇表.txt"),
    ("台湾高中",       "mahavivo/english-wordlists@master", "台灣高中英文參考詞彙表.txt"),
    ("小学",           "mahavivo/english-wordlists@master", "小学英语大纲词汇.txt"),
    ("红宝书 GRE",     "mahavivo/english-wordlists@master", "红宝书 GRE词汇精选.csv"),
    ("专八星标",       "mahavivo/english-wordlists@master", "英语专业星标八级词汇.txt"),
    ("六级星标",       "mahavivo/english-wordlists@master", "英语六级词汇（星标，1726）.txt"),

    ("四级乱序",       "KyleBing/english-vocabulary@master", "3 四级-乱序.txt"),
    ("六级乱序",       "KyleBing/english-vocabulary@master", "4 六级-乱序.txt"),
    ("托福乱序",       "KyleBing/english-vocabulary@master", "6 托福-乱序.txt"),
    ("SAT 乱序",       "KyleBing/english-vocabulary@master", "7 SAT-乱序.txt"),
    ("雅思乱序",       "KyleBing/english-vocabulary@master", "full_line_tsv/full/乱序/雅思.txt"),
    ("GMAT",           "KyleBing/english-vocabulary@master", "8 GMAT-乱序.txt"),
    ("专四",           "KyleBing/english-vocabulary@master", "9 专四-乱序.txt"),
    ("专八",           "KyleBing/english-vocabulary@master", "10 专八-乱序.txt"),

    ("JLPT N5",        "evanclan/OpenJLPT@main", "data/json/vocab/n5.json"),
    ("JLPT N4",        "evanclan/OpenJLPT@main", "data/json/vocab/n4.json"),
    ("JLPT N3",        "evanclan/OpenJLPT@main", "data/json/vocab/n3.json"),
    ("JLPT N2",        "evanclan/OpenJLPT@main", "data/json/vocab/n2.json"),
    ("JLPT N1",        "evanclan/OpenJLPT@main", "data/json/vocab/n1.json"),
    ("Anki JLPT N5",   "jamsinclair/open-anki-jlpt-decks@main", "src/n5.csv"),
]


def url_for(host, repo, path):
    from urllib.parse import quote
    return "https://%s/gh/%s/%s" % (host, repo, quote(path))


def fetch(label, repo, path):
    ctx = ssl.create_default_context()
    ctx.check_hostname = False
    ctx.verify_mode = ssl.CERT_NONE
    last = ""
    for host in HOSTS:
        u = url_for(host, repo, path)
        for _ in range(2):
            try:
                req = urllib.request.Request(u, headers={"User-Agent": UA})
                with urllib.request.urlopen(req, timeout=25, context=ctx) as r:
                    body = r.read()
                return label, repo, path, r.status, len(body), body
            except Exception as e:  # noqa: BLE001
                last = "%s %s" % (host, e)
    return label, repo, path, 0, 0, last.encode()


def main():
    with cf.ThreadPoolExecutor(max_workers=8) as ex:
        futs = [ex.submit(fetch, *c) for c in CANDS]
        for f in cf.as_completed(futs):
            label, repo, path, status, size, body = f.result()
            if not status:
                print("  [FAIL] %-14s %s  %s" % (label, body.decode("utf-8", "ignore")[:90], path))
                continue
            try:
                txt = body.decode("utf-8")
            except UnicodeDecodeError:
                txt = body.decode("gbk", "ignore")
            lines = [l for l in txt.splitlines() if l.strip()]
            sample = " | ".join(l[:60] for l in lines[:2])
            print("  [OK %3d] %-14s %8d B  约 %5d 行  %s" % (status, label, size, len(lines), sample[:100]))


if __name__ == "__main__":
    main()
