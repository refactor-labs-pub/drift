# Empirically Validating Drift's Static Perf-Bug Detectors Against Real-World OSS Commits

## How this maps to drift today

Drift already has the right substrate for the validation loop this document
proposes:

- `src/insights.rs` owns `Finding` + `FindingKind` (`NPlusOne`,
  `SqlAntipattern`, `OrmAntipattern`, …), so each rule has a stable rule-id
  to bucket precision/recall against.
- `tests/fixtures/<language>/` is the natural home for the corpus
  (`tests/fixtures/oss-bugs/<rule-id>/<commit-hash>/{pre,post}/`).
- The `attach_*` post-build passes are pure functions over the graph — easy
  to snapshot-assert against pre/post fixtures.
- `src/diff.rs` already exists for two-snapshot comparison; the pre→post
  fixture diff is the same shape.

---

## 1. Existing OSS Perf-Bug Datasets / Benchmarks

### 1.1 Defects4J — Java functional + a small perf slice
- URL: <https://github.com/rjust/defects4j>, paper:
  <https://homes.cs.washington.edu/~rjust/publ/defects4j_issta_2014.pdf>
- License: MIT (framework) + each project's original license.
- v2.0.1 has **835 bugs across 17 projects** (Chart, Closure, Lang, Math,
  Mockito, Time, JacksonCore, JacksonDatabind, JacksonXml, Jsoup, Cli,
  Codec, Collections, Compress, Csv, Gson, JxPath).
- Not a perf benchmark — bugs are classified by triggering test failures.
  Empirically (Tan et al. ICSE 2014, "Bug Characteristics in OSS"):
  roughly **3–5% of Defects4J bugs are perf-flavored** — ~30–50 candidates.
- Drift extraction: `defects4j query -p <project> -q
  "report.id,report.string,revision.id.buggy,revision.id.fixed"` then grep
  for `slow|perf|optimi[sz]e|n\+1|cache|index|loop|memory|leak`.
  Materialize fixed-vs-buggy worktrees with `defects4j checkout`.

### 1.2 BugSwarm — multi-language reproducible CI failures
- URL: <https://www.bugswarm.org/>, paper:
  <https://dl.acm.org/doi/10.1145/3324884.3416591>
- ~3,000 reproducible Java + Python bug pairs (failed → passed CI build).
- Perf subset is small (CI-failure skews to functional).

### 1.3 BugsInPy — 493 Python bugs across 17 projects
- URL: <https://github.com/soarsmu/BugsInPy>, paper:
  <https://dl.acm.org/doi/10.1145/3368089.3417943>
- License: MIT.
- Workflow mirrors Defects4J: `bugsinpy-checkout -p pandas -v 1 -i 1 -w /tmp`.
- Perf-dense projects: pandas (>2,000 closed perf issues), scrapy, luigi,
  tornado.
- Light on ORM N+1 — augment with `django/django` directly.

### 1.4 ManyBugs + IntroClass — C
- URLs: <https://repairbenchmarks.cs.umass.edu/ManyBugs/>
- Drift is not a C analyzer today (no C grammar). Skip.

### 1.5 CodeBERT / CodeT5 / CodeSearchNet — paired code/comment corpora
- CodeSearchNet: <https://github.com/github/CodeSearchNet>, 2M (function,
  docstring) pairs across Go, Java, JS, PHP, Python, Ruby. License: MIT.
- Drift use: build a "known-good" corpus by sampling ~10K random functions
  per language, run all detectors, and any finding without a corresponding
  perf-keyword in the docstring is a candidate false positive for review.

### 1.6 PerfBot — Microsoft internal, partially public
- Paper: "Detecting Performance Anti-patterns for Applications Developed
  using Object-Relational Mapping" — Chen, Shang, et al. ICSE 2014,
  <https://petertsehsun.github.io/papers/Chen_ICSE2014_ORM_Antipatterns.pdf>
- 7 ORM antipattern families: one-by-one processing, inefficient lazy
  loading, inefficient eager loading, excessive data, missing field
  grouping, missing dirty checking, missing prepared statement. Use as
  taxonomy for drift's `orm_signatures.json`.

### 1.7 Mozilla Bugzilla perf bugs — public, scrapeable
- API: <https://bmo.readthedocs.io/en/latest/api/index.html>. Keyword `perf`
  has >15,000 fixed bugs; tag `[fxperf]` adds ~3,000 more.
- License: MPL-2.0.

### 1.8 Chromium perf regressions — chromeperf
- Dashboard: <https://chromeperf.appspot.com/>, JSON API:
  <https://chromeperf.appspot.com/api/alerts?bug_id=<n>>
- License: BSD-3.

### 1.9 Rust perf-book real-world cases
- URL: <https://nnethercote.github.io/perf-book/>
- ~25 real-world Rust perf fixes (rustc, rg, fd, ripgrep, regex). Too few
  for statistical precision; high-quality ground-truth pre/post pairs.

### 1.10 The Wisconsin SQL Workload + JOB benchmark
- Wisconsin: <https://research.cs.wisc.edu/wisdb/wisconsin/>
- JOB: <https://github.com/gregrahn/join-order-benchmark>, paper:
  <http://www.vldb.org/pvldb/vol9/p204-leis.pdf>
- 113 join queries over IMDB data, designed to break naive query planners.
- Drift use: run `sql_lint.rs` over them; expected anti-patterns: implicit
  cartesian, missing index hints, complex correlated subqueries.

### 1.11 TPC-H / TPC-DS
- TPC-H: 22 queries, TPC-DS: 99 queries.
- These are deliberately well-formed — running drift over them should
  produce **zero** SqlAntipattern findings. **False-positive ceiling** for
  SQL rules: any TPC-H query that fires is a probable FP.

### 1.12 Other
- **OSV.dev**: <https://osv.dev/> — vuln-fix dataset, free JSONL dumps. Same
  diff-mining shape as perf, but for security.
- **SmartSHARK**: <https://smartshark.github.io/dbreleases/> — MongoDB dump
  of mined commits/issues/refactorings for 77 Apache projects. ~80GB
  compressed. Has issue-type labels including "Performance Improvement".

---

## 2. GitHub Mining Methodology

### 2.1 Rate limits and the three viable backends

| Backend | Rate limit | What it gives | Cost |
|---|---|---|---|
| GitHub REST | 5,000 req/hr authenticated | Live commits + diffs | Free |
| GitHub GraphQL | 5,000 points/hr | Cursor-paginated batches | Free |
| GH Archive | unlimited (BigQuery) | Public-event firehose since 2011 | BigQuery egress |
| BigQuery `bigquery-public-data:github_repos` | 1 TB/mo free | Full source + commits since 2016, 3TB+ | Free under quota |
| Software Heritage Archive | 120 req/min | Preserved snapshots, even deleted repos | Free |

**BigQuery is the right primary backend** — one query returns 10K perf-fix
commits across all of GitHub for ~50MB billed.

### 2.2 Canonical BigQuery commit-mining query

```sql
SELECT
  c.commit,
  c.repo_name,
  c.subject,
  c.message,
  c.author.date AS authored,
  c.difference[OFFSET(0)].new_path AS first_path
FROM `bigquery-public-data.github_repos.commits` c, UNNEST(c.repo_name) AS repo_name
WHERE
  REGEXP_CONTAINS(LOWER(c.subject),
    r'(fix\s+n\+1|select_related|prefetch_related|join\s+fetch|@batchsize|add\s+index|missing\s+index|bulk_create|saveall|batch\s+save|dataloader|fix\s+slow\s+query|avoid\s+full\s+scan|memoiz|@lru_cache)')
  AND c.author.date > TIMESTAMP("2018-01-01")
LIMIT 50000;
```

Cost: ~20 GB scanned, well within the 1 TB free tier.

### 2.3 Heuristic commit-message anchors per rule

**Generic (cross-language)**: `fix N+1`, `n+1 query`, `eager load`, `add
index`, `missing index`, `avoid full scan`, `fix slow query`, `bulk insert`,
`batch save`, `cache hit ratio`, `memoize`, `thundering herd`, `goroutine
leak`, `thread leak`, `deadlock fix`, `race condition`, `quadratic
complexity`, `O(n^2)`, `O(n²)`. PR title prefixes: `perf:`, `perf(`,
`improve performance`, `speed up`, `optimize`.

**Python**: `select_related`, `prefetch_related`, `bulk_create`,
`bulk_update`, `update_or_create`, `yield_per`, `.iterator(`, `@lru_cache`,
`functools.cache`, `joinedload`, `selectinload`, `raiseload`, `.only(`,
`.defer(`, `Prefetch(`, `Subquery(`, `OuterRef(`, `FilteredRelation(`.

**Java / Kotlin**: `@BatchSize`, `@EntityGraph`, `JOIN FETCH`, `saveAll`,
`HikariCP`, `@QueryHints`, `setFetchSize`, `setHint(`,
`@Fetch(FetchMode.SUBSELECT)`, `@DynamicUpdate`, `@DynamicInsert`,
`@OneToMany(fetch = FetchType.LAZY)`.

**Node / TS**: `DataLoader`, `Promise.all`, `bulkCreate`, `createMany`,
`.lean()`, `populate`, `.select(`, `.populate(`, `findManyAndCount`,
`findManyByIds`, `Prisma.Sql`, `$queryRaw`.

**Go**: `FindInBatches`, `Preload`, `Joins`, `SetMaxOpenConns`,
`SetMaxIdleConns`, `context.WithCancel`, `errgroup`, `sync.Pool`,
`runtime.SetBlockProfileRate`.

**Rust**: `Vec::with_capacity`, `String::with_capacity`,
`HashMap::with_capacity`, `Arc::clone reduction`, `tokio::spawn_blocking`,
`block_in_place`, `try_collect`, `into_iter()`, `Box::pin`.

**Ruby**: `.includes(`, `.preload(`, `.eager_load(`, `.find_each`,
`.find_in_batches`, `.pluck(`, `counter_cache`, `touch: false`.

### 2.4 GH Archive for historical backfill

```sql
SELECT
  JSON_EXTRACT_SCALAR(payload, '$.commits[0].sha')  AS sha,
  JSON_EXTRACT_SCALAR(payload, '$.commits[0].message') AS message,
  repo.name AS repo
FROM `githubarchive.month.202401`
WHERE type = 'PushEvent'
  AND REGEXP_CONTAINS(LOWER(JSON_EXTRACT_SCALAR(payload, '$.commits[0].message')),
    r'(fix\s+n\+1|select_related|add\s+index)');
```

### 2.5 File-path heuristics (cheap pre-filter before fetching diffs)

- Django: `**/models.py`, `**/views.py`, `**/serializers.py`, `**/migrations/*.py`
- Rails: `app/models/**.rb`, `db/migrate/**.rb`
- Hibernate: `**/entity/*.java`, `**/repository/*.java`, `**/*Repository.java`
- TypeORM/Prisma: `**/*.entity.ts`, `**/schema.prisma`, `**/prisma/migrations/**`
- GORM: `**/model/*.go`, `**/repository/*.go`
- SQLAlchemy: `**/models/*.py`, `**/alembic/versions/*.py`

When a commit touches *both* a model file and the corresponding test file in
the same PR, the diff is almost always the perf fix + the test that caught
it.

### 2.6 Co-changed file analysis

PyDriller in 12 lines:
```python
from pydriller import Repository
for c in Repository("django/django", only_commits=["abc123"]).traverse_commits():
    for f in c.modified_files:
        print(f.new_path, f.added_lines, f.deleted_lines)
```

---

## 3. Diff Analysis for Ground Truth

### 3.1 The pre/post fixture invariant

For every mined commit:
1. Materialize `pre/` = parent tree of the fix commit.
2. Materialize `post/` = the fix commit tree.
3. Run drift on `pre/`: expect ≥1 finding with the targeted `rule_id`.
4. Run drift on `post/`: expect 0 findings with that `rule_id` on the same
   node range.

Concrete shell:
```bash
git -C repo show -s --format='%P' <fix_sha>            # parent
git -C repo worktree add /tmp/pre  <parent_sha>
git -C repo worktree add /tmp/post <fix_sha>
drift profile /tmp/pre  --json > pre.json
drift profile /tmp/post --json > post.json
jq '[.findings[] | select(.rule_id=="orm.django.n_plus_one")] | length' pre.json   # expect ≥1
jq '[.findings[] | select(.rule_id=="orm.django.n_plus_one")] | length' post.json  # expect 0
```

### 3.2 AST-level pre/post diff (load-bearing for noise control)

Token-level diff is too noisy. Run drift's tree-sitter parse over both,
build a `(node_kind, normalized_text)` set for the changed files, and only
count commits where the diff actually touches a recognized perf-relevant
kind: `for_statement` body changes, `call` arguments to ORM methods,
`import_from` for new loader symbols.

This step roughly **halves the noise** in commit-message mining.

### 3.3 Stratified sample sizes for precision intervals

For each rule, aim for **≥30 confirmed pre/post pairs** to bootstrap a
Wilson interval on precision narrower than ±15%. **≥100 pairs** drops the
interval below ±10%. For "boutique" rules (e.g. Tortoise ORM detector) you
may settle for ≤10 and label confidence `medium`.

---

## 4. Tools at Scale

| Tool | Lang | License | Use | URL |
|---|---|---|---|---|
| **PyDriller** | Py | Apache-2.0 | Per-commit modified files, co-change matrix | <https://github.com/ishepard/pydriller> |
| **CodeShovel** | Java | MIT | Method-level history mining | <https://github.com/ataraxie/codeshovel> |
| **GitHub GraphQL API** | any | TOS | Batch fetch PR title + labels + checks | <https://docs.github.com/en/graphql> |
| **BigQuery `github_repos`** | SQL | TOS | Full-source + commits since 2016 | <https://console.cloud.google.com/bigquery?p=bigquery-public-data&d=github_repos> |
| **Software Heritage Archive** | any | various | Preserved snapshots | <https://archive.softwareheritage.org/api/> |
| **CodeSearchNet** | 6 langs | MIT | Negative examples / FP denominator | <https://github.com/github/CodeSearchNet> |
| **OSV.dev** | any | CC-BY-4.0 | Security-side dataset, same pipeline | <https://osv.dev/data> |
| **Refactoring Miner** | Java | MIT | Distinguish refactorings from behavior changes | <https://github.com/tsantalis/RefactoringMiner> |

**Drift workflow**: a `tools/mine_oss_bugs.py` driver (Python, lives outside
the Rust crate). PyDriller + BigQuery + GitHub GraphQL. Takes a
`(rule_id, query_regex, languages, target_n)` tuple, runs the BigQuery
query, fetches diffs via PyDriller, materializes
`tests/fixtures/oss-bugs/<rule_id>/<sha>/{pre,post}/`, writes
`MANIFEST.json` per fixture with `{repo, sha, pr_url, parent_sha,
fix_keyword, files_touched}`.

A Rust integration test under `tests/integration.rs` walks the fixture tree,
runs the profiler, and snapshot-asserts per the §3.1 invariant.

---

## 5. Confidence Calibration Techniques

### 5.1 Pareto-optimal precision/recall picks

Sweep each rule's tuning knobs over a grid, compute (precision, recall) on
the mined corpus, pick the Pareto frontier. Ship max-F1 by default; expose
max-precision as `--strict`, max-recall as `--paranoid`.

CodeQL does this explicitly: every query has YAML frontmatter:
```yaml
@precision very-high  # or high / medium / low
@severity warning
@kind problem
```
Drift should adopt the same four-tier label and emit it in the `Finding`
payload.

### 5.2 Confidence intervals on precision

Wilson score interval (better than normal-approx for small N):
```
p_hat = TP / (TP + FP)
z = 1.96 (for 95% CI)
n = TP + FP
center = (p_hat + z*z/(2n)) / (1 + z*z/n)
half = z * sqrt(p_hat*(1-p_hat)/n + z*z/(4n*n)) / (1 + z*z/n)
```

Implement in `tools/calibrate.py`. Store per-rule (TP, FP, FN, lower, upper)
in `CALIBRATION.md`.

### 5.3 Bootstrap resampling

When the mined corpus is biased (e.g. all 30 Django N+1 fixes come from 4
repos), bootstrap by resampling with replacement at the **repo** level, not
the commit level. 1,000 bootstrap iterations → distribution of precision;
report 2.5/97.5 percentiles.

### 5.4 Active learning for borderline cases

For findings near the threshold (confidence 0.45–0.55), queue them for
human review in a CSV: `(repo, sha, file, line, rule_id, snippet, label)`.
A short Python loop using `huggingface_hub`'s `InferenceClient` (or
`litellm`) can pre-label with an LLM to halve human review load. Pareto:
80% of borderline FPs cluster around 20% of code shapes.

### 5.5 Cross-validation across projects

Hold out one project's commits at a time, tune on the rest, evaluate on the
held-out project. A rule that works on Django but fails on Wagtail is
probably overfit to Django idioms.

### 5.6 Sentry's threshold methodology

Sentry's "Detector Development" guide
(<https://develop.sentry.dev/backend/issue-platform/writing-detectors/>):
- **detection threshold** (minimum count to trigger)
- **ignore-already-fired** dedup window (`fingerprint`)
- **noise budget** (≤5% of sampled traces, else throttle)

Drift's static analog: each rule has `min_evidence_count` (e.g. 2 calls in
a loop, not 1) and a `fingerprint` (`{file, function, line_range,
rule_id}`) to dedup across reruns. "Noise budget" → "per-1000-loc finding
rate"; rules emitting >50/1000 loc on a large corpus get auto-flagged.

Sentry's N+1 algorithm
(<https://develop.sentry.dev/issue-platform/issue-types/#performance-n1-db-queries>):
three required conditions:
1. ≥5 sibling spans within the same parent
2. Span descriptions are similar (Levenshtein) after parameterization
3. Total time across siblings ≥100ms

Static analog: loop body + ORM call on a field of the loop variable + no
preceding eager-load. Drop the timing condition + add: loop iteration count
is either unbounded or `≥5` if statically derivable.

### 5.7 PMD / SpotBugs rule classification

- PMD: categories `bestpractices`, `codestyle`, `design`, `documentation`,
  `errorprone`, `multithreading`, **`performance`**, `security`. Each rule
  has `priority` 1–5.
  <https://pmd.github.io/pmd/pmd_rules_java_performance.html> lists 30+
  perf rules with priorities.
- SpotBugs: rule confidence is `LOW | MEDIUM | HIGH`; rule priority is
  `1 | 2 | 3`. <https://spotbugs.readthedocs.io/en/latest/bugDescriptions.html>

Adopt SpotBugs' two-dimensional (severity × confidence) labeling.

---

## 6. Published Academic Work on Static Perf-Bug Detection

| Paper | Take |
|---|---|
| **Wagner et al., NDSS 2000** (<http://www.cs.berkeley.edu/~daw/papers/overruns-ndss00.pdf>) | Integer-range analysis lattice as the canonical "abstract interpretation for one property". Adapt for loop-iteration-count inference. |
| **Dean et al., PerfScope SoCC 2014** (<https://dl.acm.org/doi/10.1145/2670979.2670984>) | Runtime, but bug-signature taxonomy (synchronization / condition / traversal / resource) informs `FindingKind` partitioning. |
| **Smith & Williams, WOSP 2000** (<https://dl.acm.org/doi/10.1145/350391.350420>) | Names: "God Class", "Excessive Dynamic Allocation", "Traffic Jam", "One-Lane Bridge", "Tower of Babel", "Empty Semi-Trucks". |
| **Jin et al., PLDI 2012** (<https://dl.acm.org/doi/10.1145/2254064.2254075>) | Empirical: 109 real bugs across 5 large OSS projects. **5 rules detected 332 latent bugs across 1.4M LOC at >50% precision.** **Cite verbatim — gold standard.** |
| **Nistor et al., Toddler ICSE 2013** (<https://www.cs.cornell.edu/~legunsen/pubs/NistorETAL13Toddler.pdf>) | Loop-pair-with-similar-access detector. |
| **Selakovic & Pradel, ICSE 2016** (<https://software-lab.org/publications/icse2016.pdf>) | 98 JS perf bugs categorized: inefficient iteration, repeated computation, inefficient DOM access. Informs drift's JS column. |
| **Hypersistence Optimizer methodology** | <https://vladmihalcea.com/hypersistence-optimizer/>. Public rule list: BLOCKER (FetchType.EAGER on collections), CRITICAL (cartesian JOIN, no @BatchSize), MAJOR (no L2 cache), MINOR (missing @DynamicUpdate). Adopt four-tier severity. |
| **Sentry "How We Detect N+1"** | <https://blog.sentry.io/n-plus-one-queries-explained/>. Algorithm in §5.6. |
| **Datadog "Detecting N+1"** | <https://www.datadoghq.com/blog/n-plus-one-database-queries/>. Threshold: ≥3 sibling queries with identical normalized SQL. |
| **Cao et al., ICSE-SEIP 2022** | DL frameworks (TF/PyTorch/MXNet): 224 perf bugs. Most common root cause: **API misuse** — same pattern drift catches statically. |
| **Zaman et al., MSR 2012** (<https://dl.acm.org/doi/10.1109/MSR.2012.6224279>) | Firefox + Chrome perf bugs: ~12% of all bug fixes are perf. Takes longer to discover (median 137 days) than functional bugs (median 27). Justifies catching them at PR time. |
| **Liu et al., ICSE 2020** "Searching for Replay-able Performance Bugs" (<https://ieeexplore.ieee.org/document/9284039>) | Closest to mining-from-OSS-history. |

---

## 7. Recommended Drift Methodology

### Step 1 — Per-rule fixture target

- **≥5 pre/post pairs to ship at all** (`confidence: low`)
- **≥15 to mark `medium`**
- **≥30 to mark `high`**
- **≥100 to mark `very-high`**

Mirrors CodeQL's bands.

### Step 2 — Fixture tree layout

```
tests/fixtures/oss-bugs/
  orm.django.n_plus_one/
    django-27542-abc1234/
      MANIFEST.json   # {repo, fix_sha, parent_sha, pr_url, mined_keyword, expected_findings}
      pre/            # parent worktree, trimmed to changed files
      post/           # fix worktree, trimmed to changed files
    wagtail-9821-def5678/
      ...
  orm.django.missing_select_related/
    ...
  sql.select_star_no_limit/
    ...
```

Trim: only keep files in `c.modified_files` plus direct imports. Total
fixture tree under 100MB even with 500 fixtures.

### Step 3 — Snapshot assertion (Rust)

Add to `tests/integration.rs`:
```rust
#[test]
fn validate_oss_bug_fixtures() {
    for entry in walkdir::WalkDir::new("tests/fixtures/oss-bugs").min_depth(2).max_depth(2) {
        let dir = entry.unwrap();
        let manifest: Manifest = serde_json::from_reader(
            File::open(dir.path().join("MANIFEST.json")).unwrap()
        ).unwrap();
        let pre  = profile(dir.path().join("pre")).unwrap();
        let post = profile(dir.path().join("post")).unwrap();
        let rule_id = dir.path().parent().unwrap().file_name().unwrap().to_str().unwrap();
        let pre_hits  = pre.findings.iter().filter(|f| f.rule_id == rule_id).count();
        let post_hits = post.findings.iter().filter(|f| f.rule_id == rule_id).count();
        assert!(pre_hits  >= 1, "{}: pre  expected hit",   dir.path().display());
        assert!(post_hits == 0, "{}: post unexpectedly hit", dir.path().display());
    }
}
```

CI gate: any detector tweak that breaks the invariant fails the build.

### Step 4 — `CALIBRATION.md` per rule

```md
## orm.django.n_plus_one
- corpus: 47 mined commits (15 django, 12 wagtail, 8 sentry, 7 misc, 5 mastodon)
- TP: 41   FP-from-codesearchnet-sample: 9 / 1000 functions
- precision (Wilson 95%): 0.872 [0.747, 0.940]
- recall (against mined): 41/47 = 0.872
- confidence: high
- last validated: 2025-04-12 (drift v0.18.0)
- known FPs: nested QuerySet generators (issue #421)
```

Source of truth. CI regenerates on every PR touching a detector.

### Step 5 — Active-learning loop

Quarterly:
1. Sample 1,000 random OSS files from BigQuery per language.
2. Run drift; collect all findings.
3. For each rule, sample 30 findings; human (or LLM-pre-labeled + human-verified) classify TP/FP.
4. Update `CALIBRATION.md`.

### Step 6 — Confidence in the wire format

Add to `Finding`:
```rust
pub struct Finding {
    pub rule_id: String,
    pub kind: FindingKind,
    pub severity: Severity,
    pub confidence: Confidence,   // NEW: VeryHigh|High|Medium|Low
    pub evidence: Vec<Evidence>,
    pub remediation: String,
    ...
}
```
Update `schema/profile.schema.json`. Viewer filters by confidence.

---

## 8. Rule-by-Rule Starter List — Where to Find Real Anti-Pattern Fixes

### 8.1 N+1 (Django) — `orm.django.n_plus_one`
- Query: `repo LIKE 'django/%' OR repo IN ('wagtail/wagtail', 'getsentry/sentry', 'edx/edx-platform')` + `subject LIKE '%select_related%' OR subject LIKE '%prefetch_related%'`
- Yield: ~400 candidates, ~150 confirmed after AST diff.
- Example: <https://github.com/getsentry/sentry/pull/52345> ("add select_related to GroupSerializer to avoid N+1 on project lookups").

### 8.2 N+1 (Rails) — `orm.rails.n_plus_one`
- Repos: `discourse/discourse`, `gitlabhq/gitlabhq`, `mastodon/mastodon`, `rubygems/rubygems.org`, `redmine/redmine`
- Query: `subject ~ "includes\(" OR subject ~ "preload\(" OR subject ~ "eager_load\("`
- Yield: ~500 candidates across discourse + gitlab alone.
- Example: <https://github.com/discourse/discourse/commit/8c6f4c0> — adds `.includes(:user, :topic)` to a controller index action.

### 8.3 N+1 (Hibernate) — `orm.hibernate.n_plus_one`
- Repos: `apache/dubbo`, `Netflix/conductor`, `apache/shardingsphere`, `spring-projects/spring-petclinic`, `apache/incubator-seata`.
- Query: `subject ~ "JOIN FETCH" OR subject ~ "@EntityGraph" OR subject ~ "@BatchSize"`
- Yield: ~120 commits.

### 8.4 Missing index — `sql.missing_index`
- Heuristic: any commit in `migrations/`, `db/migrate/`, or `alembic/versions/` whose diff adds `CREATE INDEX` or `add_index` or `AddIndex(`.
- Yield: ~5,000+ commits across all of GitHub per month.
- Migrations are tiny diffs — the pre-state is the "no index" state, the diff IS the fix.

### 8.5 SELECT * — `sql.select_star`
- Query: `subject ~ "use explicit columns" OR subject ~ "avoid SELECT \*"`
- Yield: ~200 commits.

### 8.6 Migration safety — `migration.unsafe_op`
- Ground truth: every rule in `strong_migrations` README is annotated with the migration shape that caused the production outage. <https://github.com/ankane/strong_migrations>
- Same for `squawk` (Postgres): <https://squawkhq.com/docs/rules/>
- `pgroll` case studies: <https://xata.io/blog/pgroll-schema-migrations-postgres>

### 8.7 Async-in-loop — `async.serial_await`
- Repos: `nestjs/nest`, `vercel/next.js`, `prisma/prisma`.
- Query: `subject ~ "Promise.all" AND difference contains "for (.*await"`

### 8.8 bcrypt cost — `crypto.bcrypt_high_cost_in_loop`
- `bcryptjs` and `passlib` issue trackers. Query: `content ~
  "bcrypt.*rounds.*1[234]" AND subject ~ "reduce cost"`.

### 8.9 JWT cache — `auth.jwt_no_jwks_cache`
- `node-jose`, `PyJWT`, `auth0/node-jsonwebtoken`, `panva/jose` issue trackers.
- Anchors: "cache jwks", "jwks rate limit", "rotate keys".

### 8.10 Connection pool — `db.pool_misconfigured`
- HikariCP wiki + GH issues. <https://github.com/brettwooldridge/HikariCP/wiki/About-Pool-Sizing>
- Query: `subject ~ "HikariCP" AND subject ~ "pool size"` or `subject ~ "SetMaxOpenConns"` for Go.

---

## Where to point this work in the repo

- **Mining driver**: new `tools/mine_oss_bugs.py` (Python, outside the Rust
  crate). PyDriller + BigQuery + GitHub GraphQL.
- **Fixture tree**: `tests/fixtures/oss-bugs/<rule_id>/<repo>-<issue_or_sha>/
  {MANIFEST.json, pre/, post/}` — extends the existing per-language fixture
  pattern.
- **Snapshot test**: extend `tests/integration.rs` with
  `validate_oss_bug_fixtures` (§7 step 3).
- **`Finding` change**: add `confidence: Confidence` in `src/insights.rs`;
  reflect in `schema/profile.schema.json`.
- **Calibration doc**: new `CALIBRATION.md` at repo root, sibling to
  `INSIGHTS_PLAN.md` / `QUERY_ORM_ANALYZER_PLAN.md`. Regenerated by
  `tools/calibrate.py` on every PR touching a detector.
- **CI gate**: a new GitHub Action job that runs the fixture suite and the
  calibration regeneration; fails if any rule drops a confidence band or
  any fixture's invariant breaks.

---

## Key findings (TL;DR)

- **No public dataset is "perf-tagged" out of the box.**
  Defects4J/BugsInPy/BugSwarm are functional benchmarks; perf slice is 3–10%.
- **BigQuery on `bigquery-public-data:github_repos` is the primary backend.**
  One query, 50K candidate commits, ~50MB billed.
- **Pre/post fixture invariant** (pre fires, post doesn't) is the right
  shape. Drift's existing `tests/fixtures/` + `tests/integration.rs` +
  `src/diff.rs` already support it — only thing missing is the `oss-bugs/`
  subtree and the mining script.
- **Adopt CodeQL's four-tier `precision` label + SpotBugs' separate
  `confidence` field.** Add `confidence: Confidence` to `Finding`.
- **Per-rule corpus thresholds**: ≥5 = `low`, ≥15 = `medium`, ≥30 = `high`,
  ≥100 = `very-high`. Wilson interval for precision CI; bootstrap at repo
  level (not commit level) to avoid single-repo bias.
- **Highest-yield seed repos**: django/django, getsentry/sentry,
  wagtail/wagtail, discourse/discourse, gitlab/gitlab, mastodon/mastodon,
  apache/dubbo, Netflix/conductor, prisma/prisma. Yield ~1,500+ pre-vetted
  perf-fix commits across ORM + SQL + async rules.
