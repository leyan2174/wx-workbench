"""
wx - 微信本地数据 CLI

自动管理 daemon 生命周期，无需用户手动启动。

用法:
  wx sessions               最近会话
  wx history "张三"          聊天记录
  wx search "关键词"         搜索消息
  wx contacts               联系人列表
  wx watch                  实时监听新消息
  wx daemon status/stop/logs daemon 管理
"""

import json
import os
import socket
import subprocess
import sys
import time

import click

CLI_DIR       = os.path.join(os.path.expanduser("~"), ".wechat-cli")
SOCK_PATH     = os.path.join(CLI_DIR, "daemon.sock")
PID_PATH      = os.path.join(CLI_DIR, "daemon.pid")
LOG_PATH      = os.path.join(CLI_DIR, "daemon.log")
DAEMON_SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "wx_daemon.py")
STARTUP_TIMEOUT = 15  # 等待 daemon 启动的最长秒数

# ─── daemon 管理 ─────────────────────────────────────────────────────────────

def _is_alive() -> bool:
    if not os.path.exists(SOCK_PATH):
        return False
    try:
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(2)
        s.connect(SOCK_PATH)
        s.sendall(b'{"cmd":"ping"}\n')
        resp = json.loads(s.makefile().readline())
        s.close()
        return resp.get("pong") is True
    except Exception:
        return False


def _start_daemon() -> None:
    subprocess.Popen(
        [sys.executable, DAEMON_SCRIPT],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    deadline = time.time() + STARTUP_TIMEOUT
    while time.time() < deadline:
        time.sleep(0.3)
        if _is_alive():
            return
    raise click.ClickException(
        f"wx-daemon 启动超时（>{STARTUP_TIMEOUT}s）\n"
        f"请查看日志: {LOG_PATH}"
    )


def _ensure_daemon() -> None:
    if not _is_alive():
        click.echo("⏳ 启动 wx-daemon...", err=True)
        _start_daemon()

# ─── 通信 ────────────────────────────────────────────────────────────────────

def _send(req: dict, timeout: int = 30) -> dict:
    _ensure_daemon()
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(timeout)
    s.connect(SOCK_PATH)
    s.sendall((json.dumps(req, ensure_ascii=False) + '\n').encode())
    resp = json.loads(s.makefile().readline())
    s.close()
    if not resp.get("ok"):
        raise click.ClickException(resp.get("error", "未知错误"))
    return resp

# ─── 时间解析 ────────────────────────────────────────────────────────────────

def _parse_time(value: str, is_end: bool = False) -> int:
    from datetime import datetime
    for fmt in ('%Y-%m-%d %H:%M:%S', '%Y-%m-%d %H:%M', '%Y-%m-%d'):
        try:
            dt = datetime.strptime(value, fmt)
            if fmt == '%Y-%m-%d' and is_end:
                dt = dt.replace(hour=23, minute=59, second=59)
            return int(dt.timestamp())
        except ValueError:
            continue
    raise click.BadParameter(
        f"无法解析时间 '{value}'，支持 YYYY-MM-DD / YYYY-MM-DD HH:MM / YYYY-MM-DD HH:MM:SS"
    )

# ─── CLI ─────────────────────────────────────────────────────────────────────

@click.group(context_settings={"help_option_names": ["-h", "--help"]})
@click.version_option("0.1.0", prog_name="wx")
def cli():
    """wx — 微信本地数据 CLI"""


# ─── sessions ────────────────────────────────────────────────────────────────

@cli.command()
@click.option('-n', '--limit', default=20, show_default=True, help='会话数量')
@click.option('--json', 'as_json', is_flag=True, help='输出原始 JSON')
def sessions(limit, as_json):
    """列出最近会话"""
    resp = _send({"cmd": "sessions", "limit": limit})
    data = resp.get("sessions", [])

    if as_json:
        click.echo(json.dumps(data, ensure_ascii=False, indent=2))
        return

    for s in data:
        unread  = f" \033[31m({s['unread']}未读)\033[0m" if s.get('unread', 0) > 0 else ''
        group   = ' [群]' if s['is_group'] else ''
        sender  = f"{s['last_sender']}: " if s.get('last_sender') else ''
        click.echo(f"\033[90m[{s['time']}]\033[0m \033[1m{s['chat']}\033[0m{group}{unread}")
        click.echo(f"  {s['last_msg_type']}: {sender}{s['summary']}")
        click.echo()


# ─── history ─────────────────────────────────────────────────────────────────

@cli.command()
@click.argument('chat')
@click.option('-n', '--limit', default=50, show_default=True, help='消息数量')
@click.option('--offset', default=0, help='分页偏移')
@click.option('--since', default=None, metavar='DATE', help='起始时间 YYYY-MM-DD')
@click.option('--until', default=None, metavar='DATE', help='结束时间 YYYY-MM-DD')
@click.option('--json', 'as_json', is_flag=True, help='输出原始 JSON')
def history(chat, limit, offset, since, until, as_json):
    """查看聊天记录

    \b
    示例:
      wx history "张三"
      wx history "AI群" --since 2026-04-01 --until 2026-04-15
      wx history "张三" -n 100 --offset 50
    """
    req = {"cmd": "history", "chat": chat, "limit": limit, "offset": offset}
    if since:
        req["since"] = _parse_time(since)
    if until:
        req["until"] = _parse_time(until, is_end=True)

    resp = _send(req)

    if as_json:
        click.echo(json.dumps(resp.get("messages", []), ensure_ascii=False, indent=2))
        return

    group = ' [群]' if resp.get('is_group') else ''
    click.echo(f"=== {resp['chat']}{group}  ({resp['count']} 条) ===\n")
    for m in resp.get("messages", []):
        sender = f"\033[33m{m['sender']}\033[0m: " if m.get('sender') else ''
        click.echo(f"\033[90m[{m['time']}]\033[0m {sender}{m['content']}")


# ─── search ──────────────────────────────────────────────────────────────────

@cli.command()
@click.argument('keyword')
@click.option('--in', 'chats', multiple=True, metavar='CHAT', help='限定聊天（可多次指定）')
@click.option('-n', '--limit', default=20, show_default=True)
@click.option('--since', default=None, metavar='DATE')
@click.option('--until', default=None, metavar='DATE')
@click.option('--json', 'as_json', is_flag=True)
def search(keyword, chats, limit, since, until, as_json):
    """搜索消息

    \b
    示例:
      wx search "Claude"
      wx search "deadline" --in "TeamA" --in "TeamB"
      wx search "会议" --since 2026-04-01
    """
    req = {"cmd": "search", "keyword": keyword, "limit": limit}
    if chats:
        req["chats"] = list(chats)
    if since:
        req["since"] = _parse_time(since)
    if until:
        req["until"] = _parse_time(until, is_end=True)

    resp = _send(req)
    results = resp.get("results", [])

    if as_json:
        click.echo(json.dumps(results, ensure_ascii=False, indent=2))
        return

    click.echo(f'搜索 "{keyword}"，找到 {resp["count"]} 条:\n')
    for r in results:
        sender = f"\033[33m{r['sender']}\033[0m: " if r.get('sender') else ''
        chat   = f"\033[36m[{r['chat']}]\033[0m " if r.get('chat') else ''
        click.echo(f"\033[90m[{r['time']}]\033[0m {chat}{sender}{r['content']}")


# ─── contacts ────────────────────────────────────────────────────────────────

@cli.command()
@click.option('-q', '--query', default=None, help='按名字过滤')
@click.option('-n', '--limit', default=50, show_default=True)
@click.option('--json', 'as_json', is_flag=True)
def contacts(query, limit, as_json):
    """查看联系人

    \b
    示例:
      wx contacts
      wx contacts -q "李"
    """
    resp = _send({"cmd": "contacts", "query": query, "limit": limit})
    data = resp.get("contacts", [])

    if as_json:
        click.echo(json.dumps(data, ensure_ascii=False, indent=2))
        return

    click.echo(f"共 {resp.get('total', len(data))} 个联系人（显示 {len(data)} 个）:\n")
    for c in data:
        click.echo(f"  {c['display']:<20} {c['username']}")


# ─── watch ───────────────────────────────────────────────────────────────────

@cli.command()
@click.option('--chat', default=None, help='只显示指定聊天的消息')
@click.option('--json', 'as_json', is_flag=True, help='输出 JSON lines（方便 jq 处理）')
def watch(chat, as_json):
    """实时监听新消息（Ctrl+C 退出）

    \b
    示例:
      wx watch
      wx watch --chat "AI交流群"
      wx watch --json | jq .content
    """
    _ensure_daemon()
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect(SOCK_PATH)
    s.sendall((json.dumps({"cmd": "watch"}) + '\n').encode())

    if not as_json:
        click.echo("监听中（Ctrl+C 退出）...\n", err=True)

    try:
        for line in s.makefile():
            line = line.strip()
            if not line:
                continue
            try:
                event = json.loads(line)
            except Exception:
                continue

            evt = event.get("event", "")
            if evt in ("connected", "heartbeat"):
                continue

            # 过滤指定聊天
            if chat and event.get("chat") != chat and event.get("username") != chat:
                continue

            if as_json:
                click.echo(line)
                continue

            time_s   = event.get('time', '')
            chat_s   = event.get('chat', '')
            is_group = event.get('is_group', False)
            sender   = event.get('sender', '')
            content  = event.get('content', '')

            chat_part   = f"\033[36m[{chat_s}]\033[0m " if is_group else f"\033[1m{chat_s}\033[0m "
            sender_part = f"\033[33m{sender}\033[0m: " if sender else ''
            click.echo(f"\033[90m[{time_s}]\033[0m {chat_part}{sender_part}{content}")

    except KeyboardInterrupt:
        pass
    finally:
        try:
            s.close()
        except Exception:
            pass


# ─── daemon 子命令组 ──────────────────────────────────────────────────────────

@cli.group()
def daemon():
    """管理 wx-daemon"""


@daemon.command()
def status():
    """查看 daemon 运行状态"""
    if _is_alive():
        pid = open(PID_PATH).read().strip() if os.path.exists(PID_PATH) else '?'
        click.echo(f"✓ wx-daemon 运行中 (PID {pid})")
    else:
        click.echo("✗ wx-daemon 未运行")


@daemon.command()
def stop():
    """停止 daemon"""
    if not os.path.exists(PID_PATH):
        click.echo("daemon 未运行")
        return
    try:
        pid = int(open(PID_PATH).read().strip())
        import signal
        os.kill(pid, signal.SIGTERM)
        click.echo(f"✓ 已停止 wx-daemon (PID {pid})")
    except (ValueError, ProcessLookupError):
        click.echo("daemon 进程不存在，清理残留文件")
        for p in (SOCK_PATH, PID_PATH):
            try:
                os.unlink(p)
            except OSError:
                pass


@daemon.command()
@click.option('-f', '--follow', is_flag=True, help='持续输出（tail -f）')
@click.option('-n', '--lines', default=50, show_default=True, help='显示最近 N 行')
def logs(follow, lines):
    """查看 daemon 日志"""
    if not os.path.exists(LOG_PATH):
        click.echo("暂无日志")
        return
    if follow:
        import subprocess as sp
        sp.run(['tail', f'-{lines}', '-f', LOG_PATH])
    else:
        with open(LOG_PATH) as f:
            all_lines = f.readlines()
        click.echo(''.join(all_lines[-lines:]), nl=False)


# ─── 入口 ────────────────────────────────────────────────────────────────────

if __name__ == "__main__":
    cli()
