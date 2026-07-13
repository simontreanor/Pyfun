"""Register the Pyfun kernelspec: `python -m pyfun_kernel.install [--sys-prefix]`.

Writes a kernel.json that launches `python -m pyfun_kernel` with the current
interpreter, so the kernel runs in the same environment pyfun-lang and
ipykernel are installed into.
"""

import argparse
import json
import os
import sys
import tempfile

from jupyter_client.kernelspec import KernelSpecManager


def main(argv=None):
    parser = argparse.ArgumentParser(
        prog="python -m pyfun_kernel.install",
        description="Install the Pyfun Jupyter kernelspec.",
    )
    location = parser.add_mutually_exclusive_group()
    location.add_argument(
        "--user",
        action="store_true",
        help="install for the current user (the default)",
    )
    location.add_argument(
        "--sys-prefix",
        action="store_true",
        help="install into sys.prefix (use inside a virtualenv or conda env)",
    )
    location.add_argument(
        "--prefix",
        default=None,
        help="install under this prefix instead",
    )
    args = parser.parse_args(argv)

    spec = {
        "argv": [sys.executable, "-m", "pyfun_kernel", "-f", "{connection_file}"],
        "display_name": "Pyfun",
        "language": "pyfun",
    }

    with tempfile.TemporaryDirectory() as staging:
        with open(os.path.join(staging, "kernel.json"), "w", encoding="utf-8") as f:
            json.dump(spec, f, indent=2)
        prefix = sys.prefix if args.sys_prefix else args.prefix
        dest = KernelSpecManager().install_kernel_spec(
            staging,
            "pyfun",
            user=prefix is None,
            prefix=prefix,
        )
    print(f"Installed the Pyfun kernelspec to {dest}")
    print('Open a notebook and pick the "Pyfun" kernel.')


if __name__ == "__main__":
    main()
