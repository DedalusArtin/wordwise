#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
upload_github.py —— 自动把 WordWise 项目上传到 GitHub。

token 来源（按优先级）：
  1. 环境变量 GITHUB_TOKEN / GH_TOKEN
  2. 本机 hermes 配置中的已保存凭据：
       - %LOCALAPPDATA%\\hermes\\.env        中的 GITHUB_TOKEN=
       - %LOCALAPPDATA%\\hermes\\auth.json   credential_pool 中的 GITHUB_TOKEN 条目
  3. ~/.config/wordwise/github_token

用法：
  # 交互式（会询问仓库名）
  python scripts/upload_github.py

  # 指定仓库名与可见性
  python scripts/upload_github.py --repo wordwise --public

  # 只初始化本地仓库，不推送
  python scripts/upload_github.py --no-push

  # 指定提交信息
  python scripts/upload_github.py -m "feat: 初始化项目"
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

# ---------------------------------------------------------------- 常量

API = "https://api.github.com"
PROJECT_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_REPO = "wordwise"

# 需要写入的 .gitignore（若项目里没有）
DEFAULT_GITIGNORE = """\
# Rust / Cargo
/src-tauri/target/
**/*.rs.bk
Cargo.lock.bak

# 构建产物
/dist/
/build/
*.exe
*.msi
installer/output/
!installer/output/.gitkeep

# 本地数据与缓存
/wordwise-data/
*.db
*.db-wal
*.db-shm
.env
.env.local

# 编辑器
.vscode/
.idea/
*.swp
.DS_Store
Thumbs.db

# 临时文件
*.log
*.tmp
"""


# ---------------------------------------------------------------- 工具

def info(msg):
    print(f"\033[36m[信息]\033[0m {msg}")


def ok(msg):
    print(f"\033[32m[成功]\033[0m {msg}")


def warn(msg):
    print(f"\033[33m[警告]\033[0m {msg}")


def fail(msg):
    print(f"\033[31m[错误]\033[0m {msg}")


def run(cmd, cwd=None, check=True, capture=True, env=None):
    """执行命令；capture=False 时直接透传到终端。"""
    kwargs = {
        "cwd": str(cwd) if cwd else None,
        "shell": isinstance(cmd, str),
        "env": env,
    }
    if capture:
        kwargs["stdout"] = subprocess.PIPE
        kwargs["stderr"] = subprocess.PIPE
        kwargs["text"] = True
        kwargs["encoding"] = "utf-8"
        kwargs["errors"] = "replace"

    proc = subprocess.run(cmd, **kwargs)
    if check and proc.returncode != 0:
        out = (proc.stdout or "") + (proc.stderr or "")
        raise RuntimeError(f"命令失败：{cmd}\n{out.strip()}")
    return proc


# ---------------------------------------------------------------- token 读取

def token_from_env():
    for k in ("GITHUB_TOKEN", "GH_TOKEN", "GITHUB_PAT"):
        v = os.environ.get(k, "").strip()
        if v:
            return v, f"环境变量 {k}"
    return None, None


def find_hermes_dir():
    """定位 hermes 配置目录。"""
    candidates = []
    local = os.environ.get("LOCALAPPDATA") or os.environ.get("localappdata")
    if local:
        candidates.append(Path(local) / "hermes")
    home = Path.home()
    candidates += [
        home / "AppData" / "Local" / "hermes",
        home / ".hermes",
        home / ".config" / "hermes",
    ]
    for c in candidates:
        if c.is_dir():
            return c
    return None


def token_from_hermes_env(hermes_dir):
    """从 hermes/.env 读取 GITHUB_TOKEN。"""
    env_file = hermes_dir / ".env"
    if not env_file.is_file():
        return None, None
    try:
        text = env_file.read_text(encoding="utf-8", errors="ignore")
    except OSError:
        return None, None

    m = re.search(r"^\s*GITHUB_TOKEN\s*=\s*(.+?)\s*$", text, re.MULTILINE)
    if not m:
        m = re.search(r'^\s*GITHUB_TOKEN\s*=\s*["\'](.+?)["\']\s*$', text, re.MULTILINE)
    if m:
        val = m.group(1).strip().strip('"').strip("'")
        if val:
            return val, f"{env_file}"
    return None, None


def token_from_hermes_auth(hermes_dir):
    """从 hermes/auth.json 的 credential_pool 里找 GitHub 相关条目。"""
    auth = hermes_dir / "auth.json"
    if not auth.is_file():
        return None, None
    try:
        data = json.loads(auth.read_text(encoding="utf-8", errors="ignore"))
    except (OSError, json.JSONDecodeError):
        return None, None

    pool = data.get("credential_pool", {})
    # 优先 label 明确为 GITHUB_TOKEN 的条目
    for provider, entries in pool.items():
        if not isinstance(entries, list):
            continue
        for e in entries:
            label = str(e.get("label", "")).upper()
            if "GITHUB" in label or "github" in provider.lower():
                # 常见明文字段
                for key in ("secret", "api_key", "key", "token", "value"):
                    v = e.get(key)
                    if isinstance(v, str) and v.strip():
                        return v.strip(), f"{auth} ({provider}/{label})"
                # 仅存指纹时无法还原明文
                if e.get("secret_fingerprint"):
                    warn(f"{provider}/{label} 只保存了指纹，无法还原明文 token")
    return None, None


def token_from_local_config():
    p = Path.home() / ".config" / "wordwise" / "github_token"
    if p.is_file():
        v = p.read_text(encoding="utf-8", errors="ignore").strip()
        if v:
            return v, str(p)
    return None, None


def resolve_token():
    """按优先级解析 token，返回 (token, 来源描述)。"""
    for fn in (token_from_env, token_from_local_config):
        t, src = fn()
        if t:
            return t, src

    hermes = find_hermes_dir()
    if hermes:
        info(f"发现 hermes 配置目录：{hermes}")
        for fn in (token_from_hermes_env, token_from_hermes_auth):
            t, src = fn(hermes)
            if t:
                return t, src
    else:
        warn("未找到 hermes 配置目录")

    return None, None


# ---------------------------------------------------------------- GitHub API

def gh_request(method, path, token, payload=None):
    url = path if path.startswith("http") else API + path
    body = None
    headers = {
        "Authorization": f"Bearer {token}",
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": "WordWise-Uploader/1.0",
    }
    if payload is not None:
        body = json.dumps(payload).encode("utf-8")
        headers["Content-Type"] = "application/json"

    req = urllib.request.Request(url, data=body, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=45) as resp:
            raw = resp.read().decode("utf-8", errors="replace")
            return resp.status, (json.loads(raw) if raw.strip() else {})
    except urllib.error.HTTPError as e:
        raw = e.read().decode("utf-8", errors="replace")
        try:
            return e.code, json.loads(raw)
        except json.JSONDecodeError:
            return e.code, {"message": raw}
    except urllib.error.URLError as e:
        raise RuntimeError(
            f"网络请求失败：{e.reason}\n"
            "  提示：若使用代理，请设置环境变量 HTTPS_PROXY，例如\n"
            "        set HTTPS_PROXY=http://127.0.0.1:7890"
        )


def get_user(token):
    status, data = gh_request("GET", "/user", token)
    if status != 200:
        raise RuntimeError(f"token 校验失败（HTTP {status}）：{data.get('message')}")
    return data


def read_version():
    """版本号以 tauri.conf.json 为准，build.ps1 也读同一处，避免脱节。"""
    cfg = PROJECT_ROOT / "src-tauri" / "tauri.conf.json"
    try:
        data = json.loads(cfg.read_text(encoding="utf-8"))
        v = str(data.get("version", "")).strip()
        if v:
            return v
    except (OSError, json.JSONDecodeError):
        pass
    warn("读不到 tauri.conf.json 的版本号，回退为 0.0.0")
    return "0.0.0"


def create_release(token, owner, repo, tag, name, body, prerelease=False):
    """创建 Release。若 tag 已存在则复用（避免重复创建导致 422）。"""
    status, data = gh_request("GET", f"/repos/{owner}/{repo}/releases/tags/{tag}", token)
    if status == 200:
        info(f"Release {tag} 已存在，复用：{data.get('html_url')}")
        return data

    info(f"创建 Release {tag} …")
    status, data = gh_request("POST", f"/repos/{owner}/{repo}/releases", token, {
        "tag_name": tag,
        "name": name,
        "body": body,
        "draft": False,
        "prerelease": prerelease,
    })
    if status not in (200, 201):
        raise RuntimeError(f"创建 Release 失败（HTTP {status}）：{data.get('message')}")
    ok(f"Release 已创建：{data.get('html_url')}")
    return data


def upload_asset(token, owner, repo, release_id, path):
    """上传一个 Release 附件。走 uploads.github.com，不走 api。"""
    path = Path(path)
    if not path.is_file():
        warn(f"附件不存在，跳过：{path}")
        return None

    # upload_url 形如 https://uploads.github.com/repos/o/r/releases/1/assets{?name,label}
    url = f"https://uploads.github.com/repos/{owner}/{repo}/releases/{release_id}/assets"
    url += "?name=" + urllib.parse.quote(path.name)

    size = path.stat().st_size
    info(f"上传附件 {path.name}（{size / 1024 / 1024:.2f} MB）…")

    req = urllib.request.Request(url, data=path.read_bytes(), method="POST", headers={
        "Authorization": f"Bearer {token}",
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        "Content-Type": "application/octet-stream",
        "User-Agent": "WordWise-Uploader/1.0",
    })
    try:
        with urllib.request.urlopen(req, timeout=300) as resp:
            data = json.loads(resp.read().decode("utf-8", errors="replace") or "{}")
    except urllib.error.HTTPError as e:
        raw = e.read().decode("utf-8", errors="replace")
        # 同名附件已存在（422）→ 先删再传，保证 Release 里是最新产物
        if e.code == 422:
            warn(f"{path.name} 已存在，先删除再重传…")
            st, rel = gh_request("GET", f"/repos/{owner}/{repo}/releases/{release_id}", token)
            for a in (rel.get("assets") or []):
                if a.get("name") == path.name:
                    gh_request("DELETE",
                               f"/repos/{owner}/{repo}/releases/assets/{a['id']}", token)
            with urllib.request.urlopen(req, timeout=300) as resp:
                data = json.loads(resp.read().decode("utf-8", errors="replace") or "{}")
        else:
            raise RuntimeError(f"上传 {path.name} 失败（HTTP {e.code}）：{raw[:300]}")

    ok(f"附件已上传：{data.get('name')} ({data.get('size', 0)} bytes)")
    return data


def ensure_repo(token, owner, repo, private, description):
    status, data = gh_request("GET", f"/repos/{owner}/{repo}", token)
    if status == 200:
        info(f"仓库已存在：{data.get('html_url')}")
        return data

    info(f"创建仓库 {owner}/{repo}（{'私有' if private else '公开'}）…")
    status, data = gh_request("POST", "/user/repos", token, {
        "name": repo,
        "description": description,
        "private": private,
        "has_issues": True,
        "has_wiki": False,
        "has_projects": False,
        "auto_init": False,
    })
    if status not in (200, 201):
        raise RuntimeError(f"创建仓库失败（HTTP {status}）：{data.get('message')}")
    ok(f"仓库已创建：{data.get('html_url')}")
    return data


# ---------------------------------------------------------------- Git 操作

def git(*args, **kw):
    cmd = ["git"] + list(args)
    return run(cmd, cwd=PROJECT_ROOT, **kw)


def ensure_gitignore():
    gi = PROJECT_ROOT / ".gitignore"
    if not gi.exists():
        gi.write_text(DEFAULT_GITIGNORE, encoding="utf-8")
        ok("已生成 .gitignore")
    else:
        content = gi.read_text(encoding="utf-8", errors="ignore")
        needed = ["/src-tauri/target/", "*.db", ".env"]
        missing = [n for n in needed if n not in content]
        if missing:
            with gi.open("a", encoding="utf-8") as f:
                f.write("\n# 由 upload_github.py 补充\n")
                for m in missing:
                    f.write(m + "\n")
            ok(f".gitignore 已补充规则：{', '.join(missing)}")


def init_repo():
    ensure_gitignore()

    if not (PROJECT_ROOT / ".git").is_dir():
        info("初始化本地 Git 仓库…")
        git("init")
        git("branch", "-M", "main")
    else:
        info("本地仓库已存在")

    # 设置默认身份（若未配置）
    r = run(["git", "config", "user.name"], cwd=PROJECT_ROOT, check=False)
    if not (r.stdout or "").strip():
        git("config", "user.name", "DedalusArtin")
    r = run(["git", "config", "user.email"], cwd=PROJECT_ROOT, check=False)
    if not (r.stdout or "").strip():
        git("config", "user.email", "dedalus@users.noreply.github.com")


def commit_all(message):
    git("add", "-A")

    r = run(["git", "diff", "--cached", "--name-only"], cwd=PROJECT_ROOT, check=False)
    staged = [l for l in (r.stdout or "").splitlines() if l.strip()]
    if not staged:
        warn("没有需要提交的变更")
        return False, 0

    info(f"暂存了 {len(staged)} 个文件")
    git("commit", "-m", message)
    ok(f"已提交：{message}")
    return True, len(staged)


def push(token, owner, repo):
    remote_url = f"https://{owner}:{token}@github.com/{owner}/{repo}.git"

    r = run(["git", "remote"], cwd=PROJECT_ROOT, check=False)
    remotes = (r.stdout or "").split()
    if "origin" in remotes:
        # 用不含 token 的地址记录，token 只在本条命令中临时使用
        run(["git", "remote", "set-url", "origin",
             f"https://github.com/{owner}/{repo}.git"], cwd=PROJECT_ROOT, check=False)
    else:
        git("remote", "add", "origin", f"https://github.com/{owner}/{repo}.git")

    info("推送到 GitHub…")
    # 临时用带 token 的地址推送，失败时自动重试一次（首次推送可能需重试）
    try:
        run(["git", "push", "-u", remote_url, "main"], cwd=PROJECT_ROOT,
            check=True, capture=False)
    except RuntimeError:
        warn("首次推送失败，重试一次…")
        run(["git", "push", "-u", "--force", remote_url, "main"],
            cwd=PROJECT_ROOT, check=True, capture=False)

    ok("推送完成")


# ---------------------------------------------------------------- 主流程

def main():
    ap = argparse.ArgumentParser(
        description="把 WordWise 上传到 GitHub",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    ap.add_argument("--repo", default=DEFAULT_REPO, help=f"仓库名（默认 {DEFAULT_REPO}）")
    ap.add_argument("--public", action="store_true", help="创建公开仓库")
    ap.add_argument("--private", action="store_true", help="创建私有仓库（默认）")
    ap.add_argument("--no-push", action="store_true", help="只初始化并提交，不推送")
    ap.add_argument("-m", "--message", default=None, help="提交信息")
    ap.add_argument("--token", default=None, help="直接指定 token（覆盖自动读取）")
    ap.add_argument("--release", action="store_true",
                    help="推送后创建 GitHub Release 并上传安装包附件")
    ap.add_argument("--tag", default=None, help="Release 标签，默认 v{tauri.conf.json 的版本}")
    ap.add_argument("--asset", action="append", default=None,
                    help="要上传的附件路径，可重复；默认取 installer/output 下的安装包与便携版 zip")
    args = ap.parse_args()

    print("=" * 62)
    print("  WordWise · GitHub 自动上传")
    print("=" * 62)

    if not shutil.which("git"):
        fail("未检测到 git，请先安装 Git for Windows")
        return 1

    # 1) token
    if args.token:
        token, src = args.token, "命令行参数"
    else:
        token, src = resolve_token()

    if not token:
        fail("未能获取 GitHub token。请任选一种方式提供：")
        print("  1. 设置环境变量 GITHUB_TOKEN")
        print("  2. 在 hermes 的 .env 中配置 GITHUB_TOKEN=")
        print("  3. 写入 ~/.config/wordwise/github_token")
        print("  4. 使用 --token 参数直接传入")
        return 1

    info(f"token 来源：{src}（{token[:8]}…{token[-4:]}，长度 {len(token)}）")

    # 2) 校验
    try:
        user = get_user(token)
    except RuntimeError as e:
        fail(str(e))
        return 1
    owner = user["login"]
    ok(f"已验证身份：{owner}（{user.get('name') or '未设置昵称'}）")

    # 3) 本地仓库
    init_repo()
    msg = args.message or "release: WordWise v0.35.0\n\n基于 Rust + Tauri 的背单词与 AI 讲解一体化桌面软件。\n- 双向背诵模式（看英选中 / 看中选英）\n- 联网多源查词 + 本地大模型兜底\n- SM-2 改良记忆调度与遗忘曲线复习计划\n- 常错词自动强化记忆与详情卡\n- 可配置词典源，支持小语种扩展\n- 常驻侧边栏查词\n- Inno Setup 安装程序打包脚本"
    committed, n = commit_all(msg)

    if args.no_push:
        info("已按 --no-push 跳过推送")
        print(f"\n本地仓库就绪：{PROJECT_ROOT}")
        print("日后推送：git push -u origin main")
        return 0

    # 4) 远程仓库
    private = not args.public
    try:
        repo_info = ensure_repo(
            token, owner, args.repo, private,
            "基于本地 LM Studio 大模型的可视化背单词与 AI 讲解一体化桌面软件（Rust + Tauri）",
        )
    except RuntimeError as e:
        fail(str(e))
        return 1

    # 5) 推送
    try:
        push(token, owner, args.repo)
    except RuntimeError as e:
        fail(str(e))
        print("\n若推送失败，常见原因：")
        print("  - token 权限不足（需 Contents: Read and write）")
        print("  - 网络/代理问题：设置 HTTPS_PROXY 后重试")
        print("  - 仓库已存在同名内容：可先 git pull --rebase origin main")
        return 1

    # 6) Release（可选）
    release_url = None
    if args.release:
        version = read_version()
        tag = args.tag or f"v{version}"

        assets = args.asset
        if not assets:
            out = PROJECT_ROOT / "installer" / "output"
            assets = [
                str(out / f"WordWise-Setup-{version}.exe"),
                str(out / f"WordWise-{version}-portable.zip"),
            ]

        body = (
            f"WordWise {version}\n\n"
            "基于本地 LM Studio 大模型的可视化背单词与 AI 讲解一体化桌面软件（Rust + Tauri 2）。\n\n"
            "## 附件\n"
            "- `WordWise-Setup-<version>.exe` —— 安装程序（推荐，含卸载入口）\n"
            "- `WordWise-<version>-portable.zip` —— 便携版，解压即用\n\n"
            "## 本版要点\n"
            "- AI 讲解语言独立下拉，选择后即时生效并记住上次选择\n"
            "- 提示词层 + 输出后处理层双层保障：模型没按所选语言输出时自动翻译并替换原文\n"
            "- 代码 / 命令 / 日志 / 路径等技术内容保持原文不翻译\n"
            "- 保留「查看原文」入口便于对照\n"
            "- 查词页合并重复的语言下拉；修复搜索框无法输入与退格\n"
            "- 在线词库下载按钮修复；单词列表支持跨语言搜索中文释义\n"
            "- 主窗口改为无边框自绘标题栏，去掉重复的关闭/收起控件\n"
        ).replace("<version>", version)

        try:
            rel = create_release(token, owner, args.repo, tag,
                                 f"WordWise {version}", body)
            release_url = rel.get("html_url")
            for a in assets:
                try:
                    upload_asset(token, owner, args.repo, rel["id"], a)
                except RuntimeError as e:
                    fail(f"附件上传失败：{e}")
        except RuntimeError as e:
            fail(f"创建 Release 失败：{e}")
            print("  提示：代码已推送成功，可稍后在网页端手动创建 Release。")

    print()
    print("=" * 62)
    ok("上传完成！")
    print(f"  仓库地址：{repo_info.get('html_url', f'https://github.com/{owner}/{args.repo}')}")
    print(f"  克隆命令：git clone https://github.com/{owner}/{args.repo}.git")
    if release_url:
        print(f"  Release：{release_url}")
    print("=" * 62)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        print("\n已取消")
        sys.exit(130)
