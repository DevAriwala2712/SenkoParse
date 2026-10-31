#!/usr/bin/env python3
"""usage: rss_wrap.py RESULT_JSON cmd args...   Spawn cmd, wait4 it, write its peak RSS (KiB) to RESULT_JSON.

Why a separate process: on Linux a child's ru_maxrss includes the high-water mark of the address space it was
forked/exec'd from. The measuring harness is large (it sorts output rows), so measuring from the harness
inflates every child. This wrapper stays tiny (a ~10 MiB floor, see EXP-005-stream caveats)."""
import json, os, sys
pid = os.posix_spawnp(sys.argv[2], sys.argv[2:], os.environ)
_, status, ru = os.wait4(pid, 0)
open(sys.argv[1], "w").write(json.dumps({"status": status, "maxrss_kib": ru.ru_maxrss}))
