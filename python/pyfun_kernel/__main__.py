"""Entry point Jupyter launches: `python -m pyfun_kernel -f {connection_file}`."""

from ipykernel.kernelapp import IPKernelApp

from .kernel import PyfunKernel

IPKernelApp.launch_instance(kernel_class=PyfunKernel)
