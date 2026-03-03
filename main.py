"""
WeChat Decrypt 一键启动

python main.py          # 提取密钥 + 启动 Web UI
python main.py decrypt  # 提取密钥 + 解密全部数据库
"""
import json
import os
import subprocess
import sys

import functools
print = functools.partial(print, flush=True)


def check_wechat_running():
    """检查微信是否在运行，返回 True/False"""
    r = subprocess.run(
        ["tasklist", "/FI", "IMAGENAME eq Weixin.exe", "/FO", "CSV", "/NH"],
        capture_output=True, text=True,
    )
    for line in r.stdout.strip().split("\n"):
        if "Weixin.exe" in line:
            return True
    return False


def ensure_keys(keys_file):
    """确保密钥文件存在，不存在则自动提取"""
    if os.path.exists(keys_file):
        with open(keys_file) as f:
            keys = json.load(f)
        if keys:
            print(f"[+] 已有 {len(keys)} 个数据库密钥")
            return

    print("[*] 密钥文件不存在，正在从微信进程提取...")
    print()
    from find_all_keys import main as extract_keys
    extract_keys()
    print()

    # 提取后再次检查
    if not os.path.exists(keys_file):
        print("[!] 密钥提取失败")
        sys.exit(1)
    with open(keys_file) as f:
        keys = json.load(f)
    if not keys:
        print("[!] 未能提取到任何密钥")
        print("    可能原因：选择了错误的微信数据目录，或微信需要重启")
        print("    请检查 config.json 中的 db_dir 是否与当前登录的微信账号匹配")
        sys.exit(1)


def main():
    print("=" * 60)
    print("  WeChat Decrypt")
    print("=" * 60)
    print()

    # 1. 加载配置（自动检测 db_dir）
    from config import load_config
    cfg = load_config()

    # 2. 检查微信进程
    if not check_wechat_running():
        print("[!] 未检测到微信进程 (Weixin.exe)")
        print("    请先启动微信并登录，然后重新运行")
        sys.exit(1)
    print("[+] 微信进程运行中")

    # 3. 提取密钥
    ensure_keys(cfg["keys_file"])

    # 4. 根据子命令执行
    cmd = sys.argv[1] if len(sys.argv) > 1 else "web"

    if cmd == "decrypt":
        print("[*] 开始解密全部数据库...")
        print()
        from decrypt_db import main as decrypt_all
        decrypt_all()
    elif cmd == "web":
        print("[*] 启动 Web UI...")
        print()
        from monitor_web import main as start_web
        start_web()
    else:
        print(f"[!] 未知命令: {cmd}")
        print()
        print("用法:")
        print("  python main.py          启动实时消息监听 (Web UI)")
        print("  python main.py decrypt  解密全部数据库到 decrypted/")
        sys.exit(1)


if __name__ == "__main__":
    main()
