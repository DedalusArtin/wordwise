"""准备随安装包分发的语音资源（`vendor/piper` 与 `vendor/tts-voices`）。

为什么需要这个脚本
------------------
语音引擎约 22 MB、预置英文语音约 60 MB，都是二进制，不适合进 git 仓库
（`.gitignore` 已经排除 `/vendor/`）。构建「带本地神经朗读」的安装包之前
跑一次即可；不跑也能构建，只是发行包里不带这些资源，用户首次使用时要
自己下载（而引擎下载实测只有约 21 KB/s，会很难受）。

用法
----
    python scripts/fetch_tts_vendor.py              # 引擎 + 预置语音（默认）
    python scripts/fetch_tts_vendor.py --engine     # 只要引擎
    python scripts/fetch_tts_vendor.py --voice      # 只要预置语音
    python scripts/fetch_tts_vendor.py --force      # 已存在也重新下

下载支持断点续传：中途失败再跑一次会从中断处继续，不会从头再来。
"""
from __future__ import annotations

import argparse
import os
import shutil
import sys
import zipfile
from pathlib import Path
from urllib.error import URLError
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parent.parent
VENDOR = ROOT / "vendor"

PIPER_RELEASE = (
    "https://github.com/rhasspy/piper/releases/download/"
    "2023.11.14-2/piper_windows_amd64.zip"
)
# 直连永远第一（能通时最快），后面是公开反代。实测 gh-proxy 约 21 KB/s，
# 所以这个脚本本身就是「构建时一次性下载」而不是「运行时下载」的理由。
GH_MIRRORS = [
    "",
    "https://gh-proxy.com/",
    "https://ghproxy.net/",
    "https://ghfast.top/",
]

HF_MIRRORS = [
    "https://hf-mirror.com/rhasspy/piper-voices/resolve/main",
    "https://huggingface.co/rhasspy/piper-voices/resolve/main",
]

# 预置语音：与 src-tauri/src/tts/mod.rs 里 preset = true 的那条必须一致。
PRESET_VOICE = "en_US-amy-medium"
PRESET_HF_DIR = "en/en_US/amy/medium"


def human(n: float) -> str:
    return f"{n / 1048576:.1f} MB" if n >= 1048576 else f"{n / 1024:.0f} KB"


def download(urls: list[str], dest: Path, label: str) -> bool:
    """多镜像 + 断点续传下载。`urls` 是**完整地址**列表，按顺序尝试。"""
    dest.parent.mkdir(parents=True, exist_ok=True)
    part = dest.with_suffix(dest.suffix + ".part")

    for i, url in enumerate(urls, 1):
        start = part.stat().st_size if part.exists() else 0
        headers = {"User-Agent": "wordwise-fetch/1.0"}
        if start:
            headers["Range"] = f"bytes={start}-"
        try:
            req = Request(url, headers=headers)
            with urlopen(req, timeout=30) as resp:
                if start and resp.status == 200:
                    start = 0  # 服务器不认 Range，只能从头来
                total = int(resp.headers.get("Content-Length") or 0) + start
                mode = "ab" if start else "wb"
                got = start
                with open(part, mode) as f:
                    while True:
                        chunk = resp.read(262144)
                        if not chunk:
                            break
                        f.write(chunk)
                        got += len(chunk)
                        if total:
                            pct = got * 100 // total
                            print(
                                f"\r  {label} 源{i} {pct:3d}%  {human(got)}/{human(total)}",
                                end="",
                                flush=True,
                            )
                print()
        except (URLError, OSError, TimeoutError) as exc:
            print(f"\n  {label} 源{i} 失败：{exc}")
            continue

        if dest.exists():
            dest.unlink()
        part.replace(dest)
        print(f"  {label} 完成：{dest.relative_to(ROOT)}（{human(dest.stat().st_size)}）")
        return True

    return False


def fetch_engine(force: bool) -> bool:
    exe = VENDOR / "piper" / "piper.exe"
    if exe.exists() and not force:
        print(f"引擎已存在，跳过：{exe.relative_to(ROOT)}")
        return True

    print("下载 Piper 引擎…")
    urls = [base + PIPER_RELEASE for base in GH_MIRRORS]
    zip_path = VENDOR / "piper_windows_amd64.zip"
    if not download(urls, zip_path, "引擎"):
        print("引擎下载失败。可手动下载后解压到 vendor/piper/：")
        print(f"  {PIPER_RELEASE}")
        return False

    print("解压到 vendor/ …")
    # 包里顶层就是 piper/ 目录，所以直接解到 vendor/
    with zipfile.ZipFile(zip_path) as zf:
        zf.extractall(VENDOR)
    zip_path.unlink()
    if not exe.exists():
        print(f"解压后没找到 {exe}，包结构可能变了")
        return False
    print(f"引擎就绪：{exe.relative_to(ROOT)}")
    return True


def fetch_voice(force: bool) -> bool:
    out_dir = VENDOR / "tts-voices" / PRESET_VOICE
    onnx = out_dir / f"{PRESET_VOICE}.onnx"
    conf = out_dir / f"{PRESET_VOICE}.onnx.json"
    if onnx.exists() and conf.exists() and not force:
        print(f"预置语音已存在，跳过：{out_dir.relative_to(ROOT)}")
        return True

    print(f"下载预置语音 {PRESET_VOICE}…")
    ok = True
    for name in (f"{PRESET_VOICE}.onnx", f"{PRESET_VOICE}.onnx.json"):
        rel = f"{PRESET_HF_DIR}/{name}"
        urls = [f"{base}/{rel}" for base in HF_MIRRORS]
        if not download(urls, out_dir / name, PRESET_VOICE):
            ok = False
    if ok:
        print(f"预置语音就绪：{out_dir.relative_to(ROOT)}")
    return ok


def main() -> int:
    ap = argparse.ArgumentParser(description="准备随包语音资源")
    ap.add_argument("--engine", action="store_true", help="只下载语音引擎")
    ap.add_argument("--voice", action="store_true", help="只下载预置语音")
    ap.add_argument("--force", action="store_true", help="已存在也重新下载")
    args = ap.parse_args()

    only = args.engine or args.voice
    ok = True
    if not args.voice:
        ok = fetch_engine(args.force) and ok
    if not args.engine:
        ok = fetch_voice(args.force) and ok

    if ok:
        total = sum(f.stat().st_size for f in VENDOR.rglob("*") if f.is_file())
        print(f"\n完成，vendor/ 共 {human(total)}")
        return 0
    print("\n部分资源未就绪；构建仍可继续，只是发行包不带这些资源。")
    return 1


if __name__ == "__main__":
    sys.exit(main())
