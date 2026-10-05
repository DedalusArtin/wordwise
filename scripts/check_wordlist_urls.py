#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""
批量校验在线词库目录里的 URL 是否真实可用。

为什么需要它：
  `remote_catalog()` 里的文件名必须是源仓库里**真实存在**的。
  历史上 KAOYAN/IELTS/TOEFL/GRE/GAOKAO_edited.txt 全是 404，
  用户点「下载并导入」只会看到一串失败原因。

用法：
  python scripts/check_wordlist_urls.py            # 校验内置目录（从 Rust 源码里抓）
  python scripts/check_wordlist_urls.py url1 url2  # 校验指定 URL

做法：并发发 HEAD（失败再退回 GET，只读前 2KB）逐个探活，输出 状态码 / 字节数。
"""
import concurrent.futures as cf
import re
import ssl
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
IMPORTER = ROOT / "src-tauri" / "src" / "dict" / "importer.rs"

CTX = ssl.create_default_context()
# 直连：显式禁用代理，避免本机 HTTP_PROXY 干扰（默认配置就是直连）
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))
UA = {"User-Agent": "Mozilla/5.0 (WordWise catalog check)"}


def probe(url, timeout=20):
    req = urllib.request.Request(url, headers=UA, method="GET")
    try:
        with OPENER.open(req, timeout=timeout) as r:
            data = r.read(2048)
            total = r.headers.get("Content-Length")
            return r.status, int(total) if total and total.isdigit() else len(data), ""
    except urllib.error.HTTPError as e:
        return e.code, 0, str(e.reason)
    except Exception as e:  # 超时 / 连接被重置 / TLS 卡死
        return 0, 0, type(e).__name__


def catalog_urls():
    """从 importer.rs 的 remote_catalog() 里抠出所有 jsDelivr URL 模板。"""
    src = IMPORTER.read_text(encoding="utf-8")
    seen, out = set(), []
    for m in re.finditer(r'"(https://[^"]*jsdelivr[^"]*)"', src):
        u = m.group(1)
        if u not in seen:
            seen.add(u)
            out.append(u)
    return out


def main():
    urls = sys.argv[1:] or catalog_urls()
    if not urls:
        print("没找到要校验的 URL")
        return 1

    print(f"共 {len(urls)} 条，并发探活中…\n")
    bad = 0
    with cf.ThreadPoolExecutor(max_workers=8) as ex:
        for url, (code, size, err) in zip(urls, ex.map(probe, urls)):
            mark = "OK " if code == 200 and size > 0 else "XX "
            if mark == "XX ":
                bad += 1
            short = urllib.parse.unquote(url.split("@master/")[-1] if "@master/" in url else url)
            print(f"{mark}{code or '---':>4} {size:>9}  {short}")
            if mark == "XX ":
                print(f"        {url}")
                if err:
                    print(f"        原因：{err}")

    print(f"\n结论：{len(urls) - bad} 可用 / {bad} 不可用")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
