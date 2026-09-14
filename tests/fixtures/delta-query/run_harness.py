"""Run the real incremental archive modules with their synthetic fixtures."""
import os
from pathlib import Path
import subprocess

REPO = Path(__file__).resolve().parents[3]
LOGS = Path(os.environ.get("CARGO_TARGET_DIR", REPO / "target")) / "test-logs"


def execute(arguments, name):
    command = ["cargo", *arguments, "--offline", "--locked",
               "--target", "x86_64-pc-windows-msvc"]
    print("COMMAND:", subprocess.list2cmdline(command), flush=True)
    LOGS.mkdir(parents=True, exist_ok=True)
    log = LOGS / f"delta-query-{name}.log"
    with log.open("wb") as output:
        result = subprocess.run(command, cwd=REPO, stdout=output, stderr=subprocess.STDOUT)
        output.write(f"\nCOMMAND_EXIT_CODE={result.returncode}\n".encode())
    data = log.read_bytes()
    print(data[-4000:].decode("utf-8", errors="replace"), flush=True)
    print(f"EXIT: {result.returncode}; FULL LOG: {log}", flush=True)
    if result.returncode:
        raise SystemExit(result.returncode)


def main():
    execute(["check"], "check")
    for scope in ("export_delta", "toolkit::chat_delta", "business::archive"):
        execute(["test", "--bin", "wx", scope], scope.replace("::", "-"))


if __name__ == "__main__":
    main()
