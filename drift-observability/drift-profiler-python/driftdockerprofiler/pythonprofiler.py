"""Wall-clock profiler implementation.

Forked from Google Cloud Profiler. The collection mechanism (SIGALRM
+ setitimer + main-thread signal handler) is unchanged; what changed
is the output. The original built a gzipped pprof proto here and
returned it for upload. We now return the raw `{trace: count}` dict
and let the caller serialize.
"""

import atexit
import collections
import logging
import signal
import threading
import time
import timeit

# Maximum stack frames to record.
_MAX_STACK_DEPTH = 128
_NANOS_PER_SEC = 1000 * 1000 * 1000

logger = logging.getLogger(__name__)


class WallProfiler:
  """Python2-compatible implementation for Wall time profiler.

  This Wall profiler avoids dependency on any native code by using
  Python signal module. It only profiles the main thread. It has the following
  limitations:

  1. A system call interrupted by a signal fails with EINTR. Python doesn't
     handle EINTR properly until Python 3.5. Python 3.4 handles it in some
     modules but not all, see https://www.python.org/dev/peps/pep-0475/. In
     earlier Python, when EINTR occurs, some system calls fail with Interrupted
     Error, such as socket.recv, and some fail silently, such as sleep (Python
     doc does mention that sleep can be terminated earlier than requested:
     https://docs.python.org/2/library/time.html#time.sleep). Python assumes
     that the application code takes the responsibility to handle EINTR properly
     (usually means retry in a loop). This profiler triggers signal so it can
     cause EINTR. The risk is reduced by requiring the operation system to
     restart the interrupted system call automatically. But not all system calls
     can be restarted by the OS. See "Interruption of system calls and library
     functions by signal handlers" in
     http://man7.org/linux/man-pages/man7/signal.7.html. Using this profiler
     with Python earlier than 3.5 requires that the application code handles
     EINTR properly.
  2. The Python-side signal handling, that this profiler uses, is capable of
     executing the signal handler on the main thread only. It achieves that by
     quickly acquiring and releasing GIL in a loop until the main thread gets to
     execute (see slide 23 of http://www.dabeaz.com/python/GIL.pdf). This comes
     at expense, especially on programs with large number of threads (which are
     not that common in Python which is good news). Also this means that the
     main thread must be in a state where it can get something executed - e.g.
     it won't if it waits on a thread join.
  3. The Python signal module in Python versions older than 3.6 doesn't handle
     signals properly. Briefly speaking, when a signal arrives, a global flag
     is_tripped is used to track whether PyErr_CheckSignals is already added to
     pending calls. PyErr_CheckSignals will clear this flag when it's called. In
     some race conditions, the flag is set to 1 but PyErr_CheckSignals is not
     added to the pending calls. This causes the program to no longer handle any
     signals. For example, Ctrl + C will not kill the process. The profiler
     triggers signal with high frequency, thus makes this problem more likely to
     happen. Based on experimentation, the problem is more likely to occur in
     Python 3, so the Wall profiler is not supported for Python versions
     (inclusive) 3 to 3.5. For Python 2, the problem theoretically can happen,
     but is much less likely to manifest. Users can use it at their own risk.
  """

  PROFILE_TYPE = 'wall'

  def __init__(self, period_ms):
    """Constructs the Wall time profiler.

    Args:
      period_ms: An integer specifying the sampling interval in milliseconds.
    """
    self._period_ms = period_ms
    self._period_sec = float(period_ms) / 1000
    self._period_ns = int(self._period_sec * _NANOS_PER_SEC)
    self._traces = collections.defaultdict(int)
    self._in_handler = False
    self._started = False
    self._last_sample_time = None
    self._trace_count = 0
    self._sample_time_lock = threading.RLock()

  @property
  def period_ns(self):
    return self._period_ns

  def register_handler(self):
    """Registers the handler to the SIGALRM signal.

    This method must be called from the main thread. Attempting to call it from
    other threads will cause a ValueError exception.
    """
    signal.signal(signal.SIGALRM, self._handler)
    # Requires that the system restarts system calls interrupted by SIGALRM.
    # Not all calls can be restarted, see
    # http://man7.org/linux/man-pages/man7/signal.7.html.
    signal.siginterrupt(signal.SIGALRM, False)

    # Stop sending SIGALRM before the program exits. If SIGALRM is received
    # during the program exit, sometimes the program exits with non-zero code
    # 142. See b/133360821.
    atexit.register(signal.setitimer, signal.ITIMER_REAL, 0)

  def profile(self, duration_ns):
    """Profiles for the given duration.

    This function can be called from a non-main thread. It assumes
    register_handler has been called.

    Args:
      duration_ns: An integer specifying the duration to profile in nanoseconds.

    Returns:
      A `dict[tuple[(name, file, line), ...], int]` mapping each unique
      call stack seen during the window to its sample count. Both keys
      and values are JSON-serializable.
    """
    self._reset()

    profile_duration = float(duration_ns) / _NANOS_PER_SEC
    target_time = timeit.default_timer() + profile_duration

    self._start_profiling()
    # In Python 2, sleep can be interrupted by signal. Retries sleep until the
    # target time is reached.
    while profile_duration > 0:
      time.sleep(profile_duration)
      self._sample_time_lock.acquire()

      # Signal timer must be disabled before allocating memory. A fork call that
      # takes longer than the signal interval will enter an endless loop of
      # retrying interrupted clone system call:
      # http://lists.debian.org/debian-glibc/2010/03/msg00161.html. If that
      # happens, trying to allocation memory may hang waiting for memory lock.
      # Failing to disable the signal before allocating memory may cause a
      # deadlock.
      signal.setitimer(signal.ITIMER_REAL, 0)
      profile_duration = target_time - timeit.default_timer()
      signal.setitimer(signal.ITIMER_REAL, self._period_sec, self._period_sec)
      self._last_sample_time = None
      self._sample_time_lock.release()

    self._stop_profiling()

    return self._collect_and_clear_traces(duration_ns)

  def _record_trace(self, frame):
    """Records the call stack trace of the given frame.

    Args:
      frame: A Frame object representing the leaf frame of the stack.

    Returns:
      A tuple of frames, leaf-first. Each frame is a 5-tuple
      ``(name, file, line, qualified_name, module)``. Phase F1b
      widened the legacy 3-tuple shape; downstream consumers
      (`_filter_traces`, `frames_to_dicts`, `Builder.populate_profile`)
      are length-aware and tolerate both the 3- and 5-tuple shapes,
      so the C++ CPU sampler — which still emits 3-tuples — flows
      through unchanged.

      The two new fields enable joining a sampled profile to a
      static profile on `(file, qualified_name)`. ``qualified_name``
      is ``''`` on Python < 3.11 (``co_qualname`` is 3.11+); ``module``
      is the frame's globals' ``__name__`` or ``''`` if unavailable.
      Empty values are dropped by the writer so the JSONL stays clean.
    """
    depth = 0
    trace = []
    while frame is not None and depth < _MAX_STACK_DEPTH:
      co = frame.f_code
      # `co_qualname` is Python 3.11+. getattr with `''` keeps 3.7-3.10
      # working — those frames just don't get the field on the wire.
      qualname = getattr(co, 'co_qualname', '') or ''
      globals_ = frame.f_globals
      # f_globals is documented to always be a dict, but defensive
      # access is cheap and survives interpreter-teardown edge cases.
      module = globals_.get('__name__', '') if globals_ else ''
      frame_tuple = (co.co_name, co.co_filename, frame.f_lineno,
                     qualname, module)
      trace.append(frame_tuple)
      frame = frame.f_back
      depth += 1
    return tuple(trace)

  def _handler(self, unused_signum, frame):
    """Records the current call stack trace when signal received.

    In Python, signal can only occur between the atomic instructions of the
    Python interpreter. Since creating a string is "atomic" in Python sense,
    it's safe to copy strings in the signal handler. Also, signals are only
    handled by main thread. Simply using a bool flag to prevent reentry is
    fine.

    Args:
      frame: A Frame object representing the current stack frame.
    """

    # _started flags is used to ignore late signals that may be delivered after
    # the profiler has been stopped.
    if not self._started or self._in_handler:
      return

    self._in_handler = True
    trace = self._record_trace(frame)

    # Signal handler is only called when the execution returns to Python level
    # and when the main thread aquires the GIL. It's possible that multiple
    # signals occurred before the handler is called, for example, when Python
    # code calls into a long running C code such as gzip. The good news is that
    # we know that when the missed signal happened, the main thread Python level
    # stack is the same as the current one: if the main thread got a chance to
    # update the Python level stack, it already handled the signal. It's
    # appropriate to attribute the missed signals to the current stack.

    # Python signal handler is only called on main thread. This lock is a
    # reentrant lock which can be acquired again by the same thread without
    # blocking. We also prevented reentry of the handler function. So it's fine
    # to use this lock in the signal handler.
    self._sample_time_lock.acquire()
    now = timeit.default_timer()
    signal_tick_count = 1
    if self._last_sample_time is None:
      self._last_sample_time = now
    else:
      signal_tick_count = int((now - self._last_sample_time) / self._period_sec)
      signal_tick_count = max(1, signal_tick_count)
      self._last_sample_time += self._period_sec * signal_tick_count
    self._sample_time_lock.release()

    self._traces[trace] += signal_tick_count
    self._trace_count += signal_tick_count
    self._in_handler = False

  def _start_profiling(self):
    self._started = True
    signal.setitimer(signal.ITIMER_REAL, self._period_sec, self._period_sec)

  def _stop_profiling(self):
    """Stops timer and waits for the last handler to finish."""
    signal.setitimer(signal.ITIMER_REAL, 0)
    self._started = False

    # Waits for the last signal handler to finish.
    count = 0
    while self._in_handler:
      if count % 1000 == 0:
        logger.info('Wait for the last signal handler to finish')
      count += 1
      # This also releases the GIL to allow the main thread to finish handling.
      time.sleep(0.01)

  def _collect_and_clear_traces(self, duration_ns):
    """Return the accumulated `{trace: count}` snapshot and reset state.

    The original `_serialize_and_clear_traces` built a gzipped pprof
    proto here. We now hand back the raw dict and let the caller
    serialize however it wants.
    """
    period_ns = self._period_ns
    unknown_trace_count = int(duration_ns / period_ns) - self._trace_count
    if unknown_trace_count > 0:
      # 5-tuple `(name, file, line, qualified_name, module)` per
      # Phase F1b — matches the real-frame path so downstream
      # consumers can always positional-unpack 5 elements.
      self._traces[(('unknown', 'unknown', 0, 'unknown', 'unknown'),)] = (
          unknown_trace_count)
    traces = dict(self._traces)
    self._reset()
    return traces

  def _reset(self):
    self._traces = collections.defaultdict(int)
    self._last_sample_time = None
    self._trace_count = 0
