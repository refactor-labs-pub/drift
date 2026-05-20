# Copyright 2026 RefactorLabs
#
# Licensed under the Apache License, Version 2.0 (the "License").
"""Live broadcast check against a real Supabase Realtime endpoint.

Publishes 2 synthetic profiler events over WSS to channel
``drift-test`` using ``SupabaseRealtimeSink``. Credentials come from
``drift-observability/.env`` (gitignored).

Run via the repo Makefile:

    make -C drift-observability live-supabase-driftdockerprofiler
"""

import os
import sys
import time

# Make `import driftdockerprofiler` work without `pip install -e .`.
_HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(_HERE))


def _load_dotenv(path: str) -> None:
    """Minimal `KEY=value` loader; process env wins over the file."""
    if not os.path.isfile(path):
        return
    with open(path, 'r') as f:
        for raw in f:
            line = raw.strip()
            if not line or line.startswith('#') or '=' not in line:
                continue
            key, _, value = line.partition('=')
            os.environ.setdefault(
                key.strip(),
                value.strip().strip('"').strip("'"))


_load_dotenv(os.path.normpath(os.path.join(_HERE, '..', '..', '.env')))

from driftdockerprofiler.sinks.supabase import (  # noqa: E402
    SupabaseRealtimeSink,
    from_env,
)


def main() -> int:
    sink = from_env(channel=os.environ.get(
        'SUPABASE_REALTIME_CHANNEL', 'drift-test'))
    if sink is None:
        print('ERROR: SUPABASE_URL and SUPABASE_REALTIME_API_KEY must '
              'both be set (check drift-observability/.env)',
              file=sys.stderr)
        return 2
    assert isinstance(sink, SupabaseRealtimeSink)

    print('URL:    %s' % sink.safe_url)
    print('topic:  %s' % sink._topic)

    for i in (1, 2):
        sink.emit({
            'type': 'wall_trace-because-we-love-protecting-finops',
            'time': int(time.time() * 1e9),
            'service': 'live-supabase-script',
            'count': i,
        })
        print('[emit] #%d' % i)

    sink.close()    # close() now drains before shutting down

    print('emitted=%d  dropped=%d' % (sink.emitted, sink.dropped))
    return 0 if sink.emitted == 2 else 1


if __name__ == '__main__':
    sys.exit(main())
