# Copyright 2026 RefactorLabs
#
# Licensed under the Apache License, Version 2.0 (the "License").
"""Pluggable sampling strategies.

A *sampler* is HOW we observe the process. The legacy `WallProfiler`
(SIGALRM on the main thread) and `CPUProfiler` (SIGPROF in C++, all
threads) both already satisfy the protocol defined in `base.py` —
they are wrapped by adapters here so future samplers (the
all-threads wall sampler, a log-record sampler) can drop in behind the
same interface without touching `client.py`'s orchestration loop.
"""

from driftdockerprofiler.sampling.base import Sampler

__all__ = ['Sampler']
