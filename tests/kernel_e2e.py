"""End-to-end test of the Pyfun Jupyter kernel through jupyter_client.

Drives a real kernel process (ipykernel wrapper + `pyfun kernel-engine`) over
the Jupyter protocol — the same path JupyterLab uses — and checks the session
semantics, interrupt handling, and the engine-death replay path. Run locally
or in CI on any OS:

    pip install ipykernel jupyter_client
    PYFUN_BIN=/path/to/pyfun PYTHONPATH=python python tests/kernel_e2e.py

PYFUN_BIN defaults to `pyfun` on PATH; PYTHONPATH=python lets the kernel
package import straight from the source tree (unnecessary when pyfun-lang is
pip-installed). Exits non-zero on any failure.
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile

from jupyter_client.manager import KernelManager

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PYFUN_BIN = os.environ.get("PYFUN_BIN") or shutil.which("pyfun") or "pyfun"
if not os.path.exists(PYFUN_BIN) and os.path.exists(PYFUN_BIN + ".exe"):
    PYFUN_BIN += ".exe"

failures = []


def check(label, ok, evidence=""):
    print(f"[{'PASS' if ok else 'FAIL'}] {label}" + (f" — {evidence}" if evidence else ""))
    if not ok:
        failures.append(label)


def make_kernelspec(parent_dir):
    """Write a kernelspec that runs the kernel from this checkout."""
    spec_dir = os.path.join(parent_dir, "pyfun-e2e")
    os.makedirs(spec_dir)
    env = {"PYFUN_BIN": PYFUN_BIN}
    python_src = os.path.join(REPO, "python")
    if os.path.isdir(python_src):
        existing = os.environ.get("PYTHONPATH", "")
        env["PYTHONPATH"] = python_src + (os.pathsep + existing if existing else "")
    spec = {
        "argv": [sys.executable, "-m", "pyfun_kernel", "-f", "{connection_file}"],
        "display_name": "Pyfun (e2e)",
        "language": "pyfun",
        "env": env,
    }
    with open(os.path.join(spec_dir, "kernel.json"), "w", encoding="utf-8") as f:
        json.dump(spec, f)
    return spec_dir


def run_cell(kc, code, timeout=60):
    """Execute code; return (status, stdout, stderr)."""
    outputs = {"stdout": "", "stderr": ""}

    def hook(msg):
        if msg["header"]["msg_type"] == "stream":
            outputs[msg["content"]["name"]] += msg["content"]["text"]

    reply = kc.execute_interactive(code, output_hook=hook, timeout=timeout)
    return reply["content"]["status"], outputs["stdout"], outputs["stderr"]


def engine_pid_of(kernel_pid):
    """The pid of the kernel's `pyfun kernel-engine` child, or None."""
    if os.name == "nt":
        out = subprocess.run(
            [
                "powershell",
                "-NoProfile",
                "-Command",
                f"(Get-CimInstance Win32_Process -Filter 'ParentProcessId={kernel_pid}' "
                "| Where-Object Name -like 'pyfun*').ProcessId",
            ],
            capture_output=True,
            text=True,
        ).stdout
    else:
        out = subprocess.run(
            ["pgrep", "-P", str(kernel_pid), "-f", "pyfun"],
            capture_output=True,
            text=True,
        ).stdout
    for line in out.split():
        if line.strip().isdigit():
            return int(line.strip())
    return None


def main():
    with tempfile.TemporaryDirectory() as tmp:
        make_kernelspec(tmp)
        km = KernelManager(kernel_name="pyfun-e2e")
        km.kernel_spec_manager.kernel_dirs.insert(0, tmp)
        km.start_kernel()
        kc = km.client()
        kc.start_channels()
        try:
            kc.wait_for_ready(timeout=90)

            # --- kernel identity ---
            kc.kernel_info()
            info = kc.get_shell_msg(timeout=30)["content"]
            check(
                "kernel_info reports pyfun",
                info.get("language_info", {}).get("name") == "pyfun",
                str(info.get("language_info", {}).get("name")),
            )

            # --- session semantics ---
            status, out, _ = run_cell(kc, "let n = 6 * 7")
            check("definition echoes type", status == "ok" and "n : int" in out, out.strip())

            status, out, _ = run_cell(kc, "n + 1")
            check("expression evaluates", status == "ok" and "43" in out, out.strip())

            status, out, _ = run_cell(kc, "let double x = x * 2\ndouble n")
            check("mixed cell", status == "ok" and "84" in out, out.strip())

            status, _, err = run_cell(kc, 'let bad = n + "x"')
            check("ill-typed cell rejected", status == "error" and "error" in err, err[:60])

            status, out, _ = run_cell(kc, "n")
            check("session unchanged after error", status == "ok" and "42" in out, out.strip())

            # --- KeyboardInterrupt during a long-running cell ---
            # A CPU-bound cell: interruptible at Python bytecode level on every
            # OS. (A cell blocked in a C call like time.sleep does NOT interrupt
            # promptly on Windows — verified to be identical in the stock
            # python3 ipykernel, i.e. a platform limitation, not a Pyfun one.)
            status, out, _ = run_cell(kc, "extern pure bigRange : int -> Seq int = range")
            check("extern range accepted", status == "ok", out.strip())

            msg_id = kc.execute("Seq.fold (fun acc x -> acc + x) 0 (bigRange 2000000000)")
            # Give the cell a moment to actually be running, then interrupt.
            import time

            time.sleep(3)
            km.interrupt_kernel()
            interrupted = False
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                try:
                    reply = kc.get_shell_msg(timeout=deadline - time.monotonic())
                except Exception:
                    break
                if reply["parent_header"].get("msg_id") == msg_id:
                    interrupted = reply["content"]["status"] == "error"
                    break
            check("interrupt aborts a long cell", interrupted)

            status, out, _ = run_cell(kc, "n + 100")
            check("kernel usable after interrupt", status == "ok" and "142" in out, out.strip())

            # --- engine death: state must be rebuilt transparently ---
            kernel_pid = km.provisioner.process.pid
            engine_pid = engine_pid_of(kernel_pid)
            check("engine process found", engine_pid is not None, str(engine_pid))
            if engine_pid is not None:
                if os.name == "nt":
                    subprocess.run(
                        ["taskkill", "/PID", str(engine_pid), "/F"], capture_output=True
                    )
                else:
                    os.kill(engine_pid, 9)
                time.sleep(1)
                status, out, _ = run_cell(kc, "double (n + 8)")
                check(
                    "engine respawn replays the session",
                    status == "ok" and "100" in out,
                    f"status={status} out={out.strip()!r}",
                )
        finally:
            kc.stop_channels()
            km.shutdown_kernel(now=True)

    print("=== FAILURES ===" if failures else "=== ALL PASS ===", failures or "")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
