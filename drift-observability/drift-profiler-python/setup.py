
"""Docker Profiler Python agent packaging script.

Forked and stripped of all Google Cloud transmission. The agent now
writes JSONL events to a local file (default `/tmp/drift/events.jsonl`)
dependencies:

  - requests                  (GCE metadata server probes)

The C++ extension (`driftdockerprofiler._profiler`, the SIGPROF-based
CPU sampler) is retained verbatim: it's pure POSIX/CPython API, no
Google dependency.
"""


import glob
import re
import sys
from setuptools import Extension
from setuptools import find_packages
from setuptools import setup

# No external runtime dependencies on Python 3.8+. On 3.7, we need
# `typing_extensions` for `Protocol` / `runtime_checkable` — those
# moved into stdlib `typing` in 3.8. Conditional so 3.8+ stays
# fully dep-free.
install_requires: list = [
    'typing_extensions; python_version < "3.8"',
]

# Optional integrations stay out of the base wheel — users who never
# touch Supabase pay zero new deps. Install with:
#     pip install 'driftdockerprofiler[supabase]'
extras_require = {
    'supabase': [
        'websocket-client>=1.6',
        'certifi',
    ],
}

# --------------------------------------------------------------------- #
# C++ CPU sampler — build strategy
# --------------------------------------------------------------------- #
# Two artifacts come out of this package depending on the build host:
#
#   - Linux + CPython 3.7-3.12 → platform wheel WITH the C++ SIGPROF
#     sampler baked in (`driftdockerprofiler-X.Y.Z-cpNN-cpNN-linux_*.whl`).
#     CPU profiling works.
#
#   - Everything else (macOS / Windows / unsupported OS / Python 3.13+)
#     → pure-Python wheel (`drift_docker_profiler-X.Y.Z-py3-none-any.whl`).
#     Wall sampler (pure Python: SIGALRM + `sys._current_frames`) works
#     everywhere; CPU sampler is absent. `cpu_profiling_available()`
#     returns False, callers degrade gracefully.
#
# Each fallback path has a single reason recorded as a named "rule"
# below. Adding a new exclusion (e.g. dropping cp37 in the future) is
# one new rule, not a new `if` branch buried mid-file. Promoting an
# exclusion to "supported again" (e.g. once we port the stack walker to
# Python 3.13's renamed `_PyInterpreterFrame->f_executable` / 3.14's
# opaque frame headers) is one rule deletion.

_LINUX = sys.platform.startswith('linux')
_DARWIN = sys.platform.startswith('darwin')
_PY_VER = '%d.%d' % sys.version_info[:2]


def _cpu_sampler_extension():
  """The compiled SIGPROF CPU sampler. Returns the Extension that
  `setuptools` will compile when we decide the build host supports it.
  """
  return Extension(
      'driftdockerprofiler._profiler',
      sources=glob.glob('driftdockerprofiler/src/*.cc'),
      include_dirs=['driftdockerprofiler/src'],
      language='c++',
      extra_compile_args=['-std=c++11'],
      extra_link_args=[
          '-std=c++11',
          '-static-libstdc++',
          # libgcc_s.so.1 is preinstalled on glibc Linux but absent on
          # Alpine. Static linkage gives one wheel for both — cheaper
          # than telling Alpine users to apk add libgcc.
          '-static-libgcc',
      ])


def _rule_os_supported():
  """SIGPROF stack sampling is POSIX-only — no Windows."""
  if _LINUX or _DARWIN:
    return None
  return ('unsupported OS %r — SIGPROF stack sampling is POSIX-only. '
          'The pure-Python wall sampler still runs.' % sys.platform)


def _rule_not_darwin():
  """Apple's clang rejects the static-link flags and Darwin SIGPROF is
  too coarse — macOS gets wall-only."""
  if not _DARWIN:
    return None
  return ("macOS — Apple's clang rejects -static-libstdc++/-static-libgcc, "
          'and Darwin SIGPROF delivery is too coarse for the CPU sampler '
          'to give useful data. macOS gets the wall sampler only.')


def _rule_cpython_lt_313():
  """CPython 3.13 removed `PyThreadState->cframe` (the frame pointer
  moved back to `tstate->current_frame` after PEP 669 eliminated the
  need for the CFrame indirection) AND renamed
  `_PyInterpreterFrame->f_code` to `f_executable`.

  Both are referenced directly by
  `driftdockerprofiler/src/populate_frames.cc` — the SIGPROF stack
  walker dereferences `tstate->cframe->current_frame` and reads
  `frame->f_code`. Neither field exists on 3.13+, so the C++ ext
  literally cannot compile against 3.13's `pycore_frame.h`.

  3.14 then went further (`_PyInterpreterFrame` became opaque, public
  PyFrame_* APIs allocate + take the GIL — async-signal-unsafe), so
  the same skip-and-degrade path covers 3.14 too.

  Porting needs either an internal-header build that branches on
  PY_VERSION_HEX 0x030D0000 / 0x030E0000 with the new field names, or
  a redesign that avoids the private structs entirely. Until then,
  3.13+ users get the universal wheel — wall sampler only.
  """
  # Compare against a sentinel built at runtime so the static type
  # checker doesn't fold the comparison and flag fall-through code as
  # unreachable on whichever interpreter happens to be running setup.py.
  py_major, py_minor = sys.version_info[0], sys.version_info[1]
  if (py_major, py_minor) < (3, 13):
    return None
  return ('CPython %s — `PyThreadState->cframe` was removed in 3.13 '
          'and `_PyInterpreterFrame->f_code` was renamed to '
          '`f_executable`; the signal-safe SIGPROF stack walker '
          'references both directly and would fail to compile. '
          'Wall sampler still works.' % _PY_VER)


# Build-host rules in evaluation order. First non-None wins — the
# message it returns is the precise reason the C++ extension was
# dropped. To add a new constraint (e.g. drop cp3.7 someday), write one
# `_rule_*` helper and append it here. To re-enable a constraint
# (3.13/3.14 port lands), delete its rule.
_CPU_SAMPLER_RULES = (
    _rule_os_supported,
    _rule_not_darwin,
    _rule_cpython_lt_313,
)


def _cpu_sampler_skip_reason():
  """Return the first rule's skip message, or None if every rule passes."""
  for rule in _CPU_SAMPLER_RULES:
    reason = rule()
    if reason is not None:
      return reason
  return None


def _resolve_ext_modules():
  """Single source of truth for whether this build produces a platform
  wheel (with `_profiler.so`) or a universal `py3-none-any` wheel.

  Logs the decision either way so a failed CPU smoke test on the
  installed wheel can be traced back to setup.py output.
  """
  skip = _cpu_sampler_skip_reason()
  if skip is None:
    print('driftdockerprofiler: building C++ CPU sampler '
          '(Linux + CPython %s).' % _PY_VER)
    return [_cpu_sampler_extension()]
  print('driftdockerprofiler: skipping C++ CPU sampler — %s' % skip)
  print('driftdockerprofiler: pure-Python wheel will be produced '
        '(`py3-none-any`, universal, wall sampler only).')
  return []


ext_module = _resolve_ext_modules()


def get_version():
  """Read the version from __version__.py."""

  with open('driftdockerprofiler/__version__.py') as fp:
    # Do not handle exceptions from open() so setup will fail when it cannot
    # open the file
    line = fp.read()
    version = re.search(r"^__version__ = '([0-9]+\.[0-9]+(\.[0-9]+)?-?.*)'",
                        line, re.M)
    if version:
      return version.group(1)

  raise RuntimeError(
      'Cannot determine version from driftdockerprofiler/__init__.py.')


setup(
    # PyPI publish name uses dashes (`drift-docker-profiler`) to match
    # PyPI's normalized name conventions and the `refactorlab/drift`
    # repo namespace. Importers still `import driftdockerprofiler` —
    # Python module names can't contain dashes, so the on-disk package
    # directory keeps the run-together form.
    name='drift-docker-profiler',
    description='Local-file wall + CPU stack-sampling Python profiler '
                '(forked from Google Cloud Profiler, no GCP transmission)',
    long_description=open('README.md').read(),
    long_description_content_type='text/markdown',
    url='https://github.com/refactorlab/drift',
    project_urls={
        'Homepage':    'https://github.com/refactorlab/drift',
        'Source':      'https://github.com/refactorlab/drift/tree/main/drift-observability/drift-profiler-python',
        'Bug Tracker': 'https://github.com/refactorlab/drift/issues',
        'Changelog':   'https://github.com/refactorlab/drift/blob/main/drift-observability/drift-profiler-python/CHANGELOG.md',
    },
    author='ilya shusterman',
    author_email='shusterilyaman@gmail.com',
    maintainer='Refactor Labs',
    version=get_version(),
    python_requires='>=3.7',
    install_requires=install_requires,
    extras_require=extras_require,
    setup_requires=['wheel'],
    packages=find_packages(exclude=('tests', 'tests.*')),
    # Ship the JSON Schema files alongside the Python modules. Without
    # this, `pip install` strips them from the wheel and runtime
    # callers of `driftdockerprofiler.event_schema()` see FileNotFoundError.
    package_data={
        'driftdockerprofiler.schemas': ['*.json'],
    },
    include_package_data=True,
    ext_modules=ext_module,
    license='Apache License, Version 2.0',
    keywords='profiler python wall cpu stack-sampling',
    classifiers=[
        'Development Status :: 5 - Production/Stable',
        'Intended Audience :: Developers',
        'License :: OSI Approved :: Apache Software License',
        'Programming Language :: Python :: 3.7',
        'Programming Language :: Python :: 3.8',
        'Programming Language :: Python :: 3.9',
        'Programming Language :: Python :: 3.10',
        'Programming Language :: Python :: 3.11',
        'Programming Language :: Python :: 3.12',
        'Programming Language :: Python :: 3.13',
        'Programming Language :: Python :: 3.14',
    ],
)
