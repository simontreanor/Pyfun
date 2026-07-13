"""Jupyter kernel for Pyfun.

The kernel process is a normal Python process: it drives the compiler's
session engine (``pyfun kernel-engine``) for type-checking, type echoes, and
compilation, and ``exec``s the returned Python chunks in its own persistent
namespace — so Jupyter's stdout capture, ``input()`` support, and interrupts
all work exactly as they do for Python itself.

Install the kernelspec with ``python -m pyfun_kernel.install`` (after
``pip install "pyfun-lang[jupyter]"``).
"""

from .kernel import PyfunKernel

__all__ = ["PyfunKernel"]
