// Central glossary of "for dummies" tooltips. One source of truth so wording
// stays consistent across CallTree / DetailsPane / Statistics / Smells panes.
//
// Sources cited inline where the wording is non-obvious — these are the
// definitions practitioners actually use, not invented.

export const TIPS: Record<string, string> = {
  // ── Summary counts ────────────────────────────────────────────────────
  languages:
    'Programming languages detected in the analyzed source root.',
  files:
    'Number of source files we parsed (after .gitignore / .driftignore / built-in skips).',
  symbols:
    'Total functions + methods + classes defined across all files.',
  edges:
    'Total call relationships in the project. "A calls B" counts as one edge.',

  // ── Resource categories (the chips, the dots, the badges) ─────────────
  category_db:
    'Database call — ORM, raw SQL, NoSQL. Caught for SQLAlchemy, JPA/Hibernate, ' +
    'TypeORM, Prisma, Mongoose, Sequelize, Knex, psycopg2, pymongo and more.',
  category_network:
    'HTTP / gRPC / socket call — caught for requests, httpx, aiohttp, axios, ' +
    'node-fetch, got, OkHttp, Spring WebClient/RestTemplate, java.net.http.',
  category_io:
    'File system read/write. Caught for open(), fs.readFile, java.io/java.nio.',
  category_cache:
    'Cache touch — Redis, ioredis, memcached.',
  category_queue:
    'Message queue — Kafka, RabbitMQ (amqplib), BullMQ, JMS, SQS.',
  category_log:
    'Logging call. Usually noise, but flagged for completeness.',
  category_compute:
    'Pure computation — no external resource touched.',

  category_self:
    'Category of THIS symbol\'s own direct external calls. ' +
    'For example session.add() inside a method makes the method itself "db".',
  categories_reached:
    'Categories reachable through the entire call tree from here (transitive). ' +
    'Tells you "this handler eventually touches the DB" without you having to walk the tree.',

  // ── Per-symbol metrics ────────────────────────────────────────────────
  complexity:
    'Cyclomatic complexity (McCabe 1976): number of decision points (if / for / while / ' +
    'case / catch / && / ||) plus 1. Lower = easier to test. ' +
    'Thresholds: 1-4 simple, 5-9 moderate, 10-14 complex, 15+ untestable.',
  loc:
    'Lines of code in this symbol\'s body. A size proxy.',
  nesting_depth:
    'Maximum nested indentation level (if-inside-for-inside-while = 3). ' +
    'SonarQube\'s rule of thumb: keep ≤ 4 for readability.',
  parameter_count:
    'Number of formal parameters declared. > 5 often suggests refactoring ' +
    '("introduce a parameter object").',
  is_async:
    'Function uses async/await — Python "async def" or JS/TS "async function". ' +
    'Matters for the BLOCKING-IN-ASYNC smell.',
  is_recursive:
    'Symbol participates in a recursion cycle (mutual recursion or self-call). ' +
    'Detected via Tarjan strongly-connected components on the call graph.',

  // ── Fan-in / fan-out (graph) ──────────────────────────────────────────
  call_site_count:
    'Total invocations of this symbol anywhere in the project. ' +
    'Counts every line that calls it (so calling foo() three times from one function = 3).',
  callers_count:
    'Unique callers (different functions that call this symbol). Fan-in. ' +
    'High fan-in = widely-used utility; changing it ripples broadly.',
  callees_count:
    'Direct calls this symbol makes. Fan-out. ' +
    '> 15 often signals a "god function" doing too much.',
  subtree_size:
    'Total reachable symbols from here, including transitive callees. ' +
    'A static "blast radius" — how much code is involved when this entry executes.',
  pagerank:
    'PageRank score over the call graph (α = 0.85, Brin & Page). ' +
    'Symbols called by many heavily-called symbols score high. ' +
    'Useful for finding the central hubs of a codebase even without an obvious entry point.',

  // ── Tree percentages ──────────────────────────────────────────────────
  percent_total:
    'This subtree\'s share of the entry\'s total reachable symbols. ' +
    'The root is always 100%.',
  percent_parent:
    'This subtree\'s share of its DIRECT parent\'s subtree. ' +
    'Useful for spotting "this one child dominates its parent".',

  // ── Smells ────────────────────────────────────────────────────────────
  smell_n_plus_one:
    'N+1 QUERY: a database call inside a loop. ' +
    'Each iteration round-trips to the DB instead of fetching all rows at once. ' +
    'Classic performance antipattern — fix with batched / joined queries.',
  smell_blocking:
    'BLOCKING IN ASYNC: a sync I/O call (db/network/io) inside an async function ' +
    'without being awaited. The event loop is blocked, defeating async\'s entire benefit. ' +
    'Use an async library (httpx / aiohttp) or wrap with asyncio.to_thread.',
  smell_recursive:
    'RECURSIVE: this symbol is in a recursion cycle (direct or mutual). ' +
    'Make sure there\'s a base case and the depth is bounded.',

  // ── External calls + classification tiers ─────────────────────────────
  external_calls:
    'Calls whose target symbol isn\'t defined in the analyzed source — ' +
    'they go to third-party libs, the stdlib, framework code. ' +
    'These are where resource categorization gets attached.',
  in_loop:
    'This call site is inside a loop (for / while / comprehension). ' +
    'When combined with category=db/cache it produces the N+1 smell.',
  in_await:
    'This call site is wrapped in an await expression. ' +
    'Means the I/O is properly non-blocking.',
  tier_imported_module:
    'Classification tier B (STRONGEST): receiver name resolves to a known library import. ' +
    'Example: "import axios from \'axios\'" then "axios.post(...)" → network, no method-name guessing needed.',
  tier_receiver_pattern:
    'Classification tier C (MEDIUM): receiver name matches a well-known pattern ' +
    'like session / db / repo / axios / cache. Works without type info.',
  tier_method_signature:
    'Classification tier D (WEAKEST): method name alone is unambiguous. ' +
    'Only highly specific names like executeQuery, findOneAndUpdate, prepareStatement.',
  evidence:
    'Why the analyzer made this classification. Lets you verify or override.',

  // ── Hot paths ─────────────────────────────────────────────────────────
  hot_path:
    'A chain from an entry point ending at a categorized resource call. ' +
    'Static analog of a profiler "critical path" — but no runtime needed.',
  terminal_category:
    'The category of the final call in this hot path.',
  hot_path_depth:
    'How many call hops it takes from the entry to reach this resource. ' +
    'Deeper = more abstraction layers in the way.',

  // ── Statistics panels ─────────────────────────────────────────────────
  pagerank_top:
    'Top symbols by PageRank — the most "central" code. ' +
    'Refactoring these affects the most callers, so review them carefully.',
  dead_code:
    'Symbols with zero callers AND not pinned as entry points. ' +
    'Usually safe to delete (verify it isn\'t invoked dynamically / via reflection).',
  recursive_symbols:
    'Symbols in a strongly-connected component (size > 1) — direct or mutual recursion.',
  top_callers:
    'Symbols with the most unique callers (fan-in). ' +
    'These are your most-depended-on functions.',
  top_callees:
    'Symbols making the most direct calls (fan-out). ' +
    'Big numbers here often mean orchestrator or "god" functions.',

  // ── Flame graph & color modes ─────────────────────────────────────────
  flame_graph:
    'Hierarchical visualization (Brendan Gregg style). Each block = a function frame. ' +
    'Stack height = call depth. Block width = subtree size. Click any frame to zoom in.',
  flame_mode_kind:
    'Color frames by symbol type: function (blue), method (teal), class (orange).',
  flame_mode_category:
    'Color frames by resource category. Frames reaching the DB are tinted red.',
  flame_mode_complexity:
    'Color frames by cyclomatic complexity: teal (simple), blue, orange, red (complex), dark red (untestable).',
  flame_mode_smells:
    'Highlight only frames flagged as smells (N+1 / blocking / recursive). ' +
    'Everything else is dimmed.',

  // ── Kind badges ───────────────────────────────────────────────────────
  kind_function: 'A regular function (not inside a class).',
  kind_method:   'A method — function defined inside a class.',
  kind_class:    'A class definition.',
  kind_async_marker: 'This function is async (uses async / await).',

  // ── Truncation reasons ────────────────────────────────────────────────
  truncated_cycle:    'We stopped descending because this node is already on the path (cycle).',
  truncated_maxdepth: 'We stopped descending because we hit the --max-depth limit.',
};
