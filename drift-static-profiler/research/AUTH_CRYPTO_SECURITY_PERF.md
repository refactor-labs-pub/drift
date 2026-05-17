# Auth / Crypto / Security-Perf Overlap — Detector Catalog

Static-analysis catalog for drift-static-profiler covering patterns where the
security choice has a major perf cost (or vice versa). For each pattern: the
bad shape, observed cost, tree-sitter detection hook, severity rationale.
Severity scale follows the existing categories convention (Critical/High/
Medium/Low). All numbers are observed costs from public benchmarks.

---

## 1. Password Hashing in the Request Path

Password hashing is intentionally slow. The only acceptable cost on the request
path is "one verify per successful login." Any additional hash call (re-hash
during reads, list iterations, batch operations, migration code on the hot
path) is a 100ms–2s/op pathology multiplied by RPS.

### 1.1 Cost-factor too high for handler concurrency

Concrete numbers — bcrypt on modern x86_64 server CPU (single core), Java
patrickfav benchmark:
- cost 10 → ~76–85 ms
- cost 12 → ~305–407 ms (PHP 8.4 raised the language default from 10 → 12 in 2024)
- cost 14 → ~1220–1363 ms
- Each +1 to cost doubles work (logarithmic). Higher cost is *not* a setting
  to "tune up over time" without testing — cost 14 on a 4-core box caps you
  at ~3 logins/sec/core during peak.

Argon2id OWASP 2025 minimum: `m=19456 KiB (19 MiB), t=2, p=1`. Realistic
latency target on a typical web server is 100–500 ms per hash. If
concurrency = 100 and `m=64 MiB`, the hashing tier alone budgets 6.4 GiB
resident.

PBKDF2-HMAC-SHA256 OWASP 2023+ minimum: **600,000 iterations** (310,000
minimum for SHA-256-only mode). At 600k iterations on a modern CPU, expect
60–200 ms/hash.

### 1.2 Anti-pattern shapes to detect

Bad pattern 1 — hashing inside a list endpoint (re-hashing tokens on every row):
```python
for user in users:
    bcrypt.hashpw(user.refresh_token.encode(), bcrypt.gensalt(rounds=12))  # 400ms x N
```

Bad pattern 2 — sync hash in async handler (Python/Node):
```python
# FastAPI handler
async def login(req):
    bcrypt.checkpw(pw, hashed)  # SYNC inside async → blocks event loop
```
Node-equivalent: `bcrypt.compareSync()` or any `*Sync` from `bcryptjs` (the
pure-JS variant doesn't use libuv threadpool, just defers via
`setImmediate`).

Bad pattern 3 — cost factor hard-coded above default:
```go
bcrypt.GenerateFromPassword(pw, 14)  // ~1.2s/call
```

### 1.3 Tree-sitter shapes (Python example, mirrors across languages)

```scheme
; bcrypt with high cost in call
(call
  function: (attribute
              object: (identifier) @lib  (#match? @lib "^bcrypt$")
              attribute: (identifier) @method
              (#match? @method "^(hashpw|gensalt|checkpw)$"))
  arguments: (argument_list
               (keyword_argument
                 name: (identifier) @kw (#eq? @kw "rounds")
                 value: (integer) @cost
                 (#match? @cost "^(1[2-9]|[2-9][0-9])$"))))

; sync hash inside async function
(function_definition
  (async)
  body: (block
          (expression_statement
            (call function: (attribute
                              object: (identifier) @lib (#eq? @lib "bcrypt")
                              attribute: (identifier) @m
                              (#match? @m "^(hashpw|checkpw)$"))))))
```

Per-language symbol matrix (function names to anchor on):
- **Python**: `bcrypt.hashpw`, `bcrypt.checkpw`, `bcrypt.gensalt`,
  `passlib.hash.bcrypt.using(rounds=…).hash`, `argon2.PasswordHasher(
  memory_cost=…, time_cost=…, parallelism=…)`, `hashlib.pbkdf2_hmac(
  "sha256", ..., iterations)`
- **Node**: `bcrypt.hash`, `bcrypt.compare`, `bcrypt.hashSync`,
  `bcrypt.compareSync`, `bcryptjs.*Sync`, `@node-rs/argon2` `hash`/`verify`,
  `crypto.pbkdf2Sync`
- **Java**: `BCrypt.hashpw`, `BCrypt.checkpw`,
  `org.springframework.security.crypto.bcrypt.BCryptPasswordEncoder(strength)`,
  `Argon2BytesGenerator.generateBytes`, `PBKDF2WithHmacSHA256` via
  `SecretKeyFactory`
- **Go**: `golang.org/x/crypto/bcrypt.GenerateFromPassword(_, cost)`,
  `golang.org/x/crypto/argon2.IDKey(...)`
- **Rust**: `bcrypt::hash(_, cost)`,
  `argon2::Argon2::new(...).hash_password`
- **.NET**: `BCrypt.Net.BCrypt.HashPassword(_, workFactor)`,
  `Rfc2898DeriveBytes(_, _, iterations)`
- **Ruby**: `BCrypt::Password.create(secret, cost: …)`
- **PHP**: `password_hash(_, PASSWORD_BCRYPT, ['cost' => …])`,
  `password_hash(_, PASSWORD_ARGON2ID, [...])`

**Severity**: **Critical** when found inside a route handler with no caching
wrapper (login is fine; list/read endpoints are not). **High** when cost ≥ 12
for bcrypt or `iterations ≥ 600_000` PBKDF2 in handler scope.
**Confidence**: high (function names are unambiguous).

---

## 2. JWT Verification Per Request Without Cache

Benchmarks (PHP `web-token/jwt-framework` reference figures, also reproduced
by Nimbus, jose-jwt, jsonwebtoken):
- HS256 verify: ~10–20 µs (HMAC; symmetric key, trivial)
- RS256 verify: ~400–1300 µs (RSA-2048 modexp dominates)
- ES256 verify: ~600–900 µs
- EdDSA / Ed25519 verify: ~50–110 µs (and ~62× faster signing than RSA-2048)

The verification itself is fast. The pathology is **everything around it** —
PEM parsing, JWKS fetching, and HTTP round-trips.

### 2.1 Key parsing per call

Calling `serialization.load_pem_public_key(pem)` (Python `cryptography`) or
`RSA.parseKey()` per request re-decodes ASN.1 and reconstructs the BigInteger
modulus on every verify. Cost is 100–500 µs for RSA-2048 — small but
exact-equal to the verify itself, doubling your handler crypto cost.

Anti-pattern:
```python
def verify(token):
    pub = serialization.load_pem_public_key(open("pub.pem").read().encode())
    return jwt.decode(token, pub, algorithms=["RS256"])
```

Right shape: load once at startup, reuse the key object.

### 2.2 JWKS fetched per request

`auth0/node-jwks-rsa` defaults: `cache: true`, `cacheMaxEntries: 5`,
`cacheMaxAge: 600_000ms` (10 min). If callers pass `cache: false`, every
request hits the IdP's `/.well-known/jwks.json`. At even modest 200 RPS, this
DDoSes the IdP, runs you into rate limits, and adds 50–200 ms per request
from the network round-trip. Okta and Auth0 explicitly recommend caching JWKS
for hours-to-day and refreshing on verification failure (kid miss → cold
refresh).

Anti-pattern shapes:
- `jwksClient({ jwksUri: ..., cache: false })`
- `requests.get(jwks_url)` inside the verify function
- `JWKSource` built per request in Nimbus instead of `RemoteJWKSet` (built-in
  `DefaultJWKSetCache` with 5-min TTL/15-min refresh)

### 2.3 OIDC discovery per request

`/.well-known/openid-configuration` should be fetched at startup and
re-validated against `Cache-Control` headers (typically 1+ hours). Fetching
it per login (and then JWKS per token) means a 3-hop chain on every request.
Okta docs explicitly call out "background process that caches /keys on a
schedule, once a day."

### 2.4 Missing `kid` handling → trying every key

When the JWT header lacks `kid`, naive libs try each JWK in the set in
order — O(n) crypto verifies per token. Worst case with 5 keys × 1.3 ms
RS256 verify = 6.5 ms/request *just for the auth check*.

### 2.5 JWT signing in the handler

`jwt.encode()` with RS256 = 3+ ms (signing is far more expensive than
verifying for RSA). Putting this in a handler that issues short-lived tokens
per request (rather than long-lived sessions) costs ~3 ms × RPS in pure RSA
modexp. Use Ed25519 (≈ 42 µs) or HS256 if symmetric is acceptable.

### Tree-sitter detection

```scheme
; JWKS fetched inside function body (any verify-like function)
(function_definition
  name: (identifier) @fn
  (#match? @fn "(verify|authenticate|auth|validate)")
  body: (block
    (expression_statement
      (call function: (attribute
                        object: (identifier) @client
                        (#match? @client "(requests|httpx|urllib)"))))))

; new key parsing per call
(call function: (attribute
                  attribute: (identifier) @m
                  (#match? @m "^(load_pem_public_key|importKey|parseKey)$")))
```

Library name anchors:
- **Node**: `jsonwebtoken`, `jose`, `jwks-rsa`
- **Python**: `PyJWT`, `python-jose`, `authlib`
- **Java**: `com.nimbusds.jose.*`, `io.jsonwebtoken.*` (jjwt)
- **Go**: `github.com/golang-jwt/jwt/v5`,
  `github.com/lestrrat-go/jwx/v2/jwk` (has `jwk.Cache`)
- **Rust**: `jsonwebtoken`, `josekit`

**Severity**: **High** for JWKS-per-request, **Medium** for key-parse-per-call,
**High** for signing in hot handler. **Confidence**: medium (intent
inference needed — a function literally named `refresh_jwks` should not fire).

---

## 3. Crypto in Loops / Hot Paths

### 3.1 RSA key generation per request

Generating a 2048-bit RSA keypair = **1–2 seconds CPU**, 4096-bit = 5+
seconds with CPU near 100%. One reported test generated 4,022 keys in 962
CPU-seconds. ECDSA P-256 keygen is orders of magnitude faster — Teleport
reported a 77% CPU reduction switching from RSA to Ed25519 SSH host keys.
Any `RSA.generate_private_key(...)`, `crypto.generateKeyPair("rsa", ...)`,
or `KeyPairGenerator.getInstance("RSA")` call inside a handler is
**Critical**.

### 3.2 Per-row signing / verifying in batch

Anti-pattern: `for row in rows: hmac.new(key, row, sha256).hexdigest()` is
fine (HMAC-SHA256 = ~1 µs), but `for row: ed25519.sign(...)` adds 40–100 µs/
row → 4–10 ms/100 rows of unavoidable CPU. Batch by hashing the canonical
concatenation then signing once.

### 3.3 `hashlib.sha256(data).hexdigest()` per element vs running hasher

The constructor pays for object allocation + state init. For small chunks in
tight loops, instantiating once and calling `update()` repeatedly is 2–4×
faster. Python releases the GIL during `update()` only when chunks ≥ 2047
bytes — small per-element updates *don't* benefit from threading.

### 3.4 HMAC key derivation per call

`hmac.new(key, ...)` does setup work (XOR-pad expansion). For 100k+ calls,
prefer `hmac_obj.copy()` from a pre-seeded hasher (common BLAKE2 idiom).

### 3.5 X.509 cert parse per request

`x509.load_pem_x509_certificate()` does full ASN.1 DER decode + extension
parsing — order 100s of µs. Don't do it on the hot path; parse at startup
and store the `Certificate` object.

### 3.6 TLS handshake per request

Each full TLS 1.3 handshake adds ≥100 ms on local nets, 200–400 ms
cross-continent. RSA-2048 key exchange consumes 30–40% of edge-server CPU
during peak; ECDSA cuts that ~3–4×. Production targets ≥80% session
resumption (TLS 1.3 tickets, session IDs). Bad shape: `requests.get(url)`
per call (creates new TCP+TLS) instead of `session = requests.Session();
session.get(url)`.

### 3.7 PRNG anti-patterns

**Java**: `new Random()` re-seeds from `System.nanoTime()` per construction
(nano-collisions in tight loops give duplicate sequences). `new
SecureRandom()` per call: first use forces `/dev/random` seed read; reports
indicate **up to 90s** of latency at startup if the entropy pool is cold.
PMD has an open performance rule (issue #3222) for "reuse SecureRandom
instance."

**Python**: `secrets.token_urlsafe(32)` per call = fine (≤10 µs).
`os.urandom(32)` in a million-row loop = wasteful (syscall per call); batch
into one large buffer.

### Tree-sitter shapes

```scheme
; RSA keygen inside any function
(call
  function: (attribute attribute: (identifier) @m
                       (#match? @m "^(generate_private_key|generateKeyPair|GenerateKey)$"))
  arguments: (argument_list . (_) @first
                            (#match? @first "RSA|rsa")))

; hash constructor inside a for-loop
(for_statement
  body: (block
    (expression_statement
      (call function: (attribute
                        object: (call function: (attribute
                                                  object: (identifier) @h
                                                  attribute: (identifier) @ctor))
                        attribute: (identifier) @method))
      (#eq? @h "hashlib")
      (#match? @ctor "^(sha\\d+|md5|blake2b|blake2s)$")
      (#eq? @method "hexdigest"))))
```

**Severity**: **Critical** (RSA keygen in handler), **High** (hashing inside
DB loop, TLS without pool), **Medium** (PRNG construction in loop).
**Confidence**: high for keygen, medium for hash-in-loop (intentional content
hashing exists).

---

## 4. Session Anti-Patterns

### 4.1 Session lookup hits DB instead of cache

Redis/Memcached: sub-millisecond P99. Postgres session lookup: 5–20 ms +
connection pool wait. Cache vs DB delta on every authenticated request × all
RPS is the largest per-request win after JWKS caching.

### 4.2 Session serialization via pickle

`pickle` is a CVE waiting to happen (RCE on tampered payload) and is also
slow vs `msgpack`/`orjson`. Django's signed cookies pickling has been
deprecated for exactly this reason. Bandit B301 (`pickle`) flags this.

### 4.3 Session size > 4KB → cookie chunking

All browsers enforce 4 KB per cookie (name+value+attrs); ~8 KB total per
domain. Once you exceed 4 KB, libraries like `next-auth` and Flask sign+split
into `session.0`, `session.1`. With ~100 requests/page and 4 KB cookies,
you transmit ~400 KB extra per page load. Base64 encoding adds 10–30%
overhead. Target: keep session ≤ 1 KB; spill to server-side store beyond
that.

### 4.4 Sticky-session router patterns

Sticky sessions (load balancer pins user → backend) tie auth state to a
specific pod, breaking autoscaling and zero-downtime deploys; Redis-backed
sessions eliminate the need.

### Tree-sitter detection

```scheme
; pickle.loads/dumps in session module
(call function: (attribute
                  object: (identifier) @lib (#eq? @lib "pickle")
                  attribute: (identifier) @m (#match? @m "^(loads?|dumps?)$")))
```

**Severity**: **High** (DB session lookup per request), **Critical** (pickle
for session — security AND perf), **Medium** (cookie size warning).
**Confidence**: medium.

---

## 5. OAuth/OIDC Patterns

### 5.1 OIDC discovery endpoint per request

Discovery should be fetched at startup and refreshed per `Cache-Control`
(typically hourly). Doing it per login is a 50–200 ms round-trip tax.

### 5.2 Token introspection per request vs JWT self-validation

RFC 7662 introspection = network round-trip to authorization server per
request. JWT self-validation = local crypto, ~50 µs–1.3 ms depending on
algorithm. Recommended caching: 3–5 minutes (never beyond `exp`). Keycloak
docs explicitly warn introspection "can be slow and possibly overload the
server."

### 5.3 Refresh token rotation timing

Rotating too aggressively (per-request) creates synchronous DB writes on
every API call. Rotate on use of refresh, not on use of access token.

### 5.4 PKCE verifier hash

S256 challenge = `BASE64URL(SHA256(verifier))`. SHA-256 of 43–128 chars is
~1 µs — negligible per request. Don't surface this as a finding.

### Detection anchors

Function names: `discover`, `introspect`, `well_known`, `openid-configuration`.
Look for `requests.get` / `httpx.get` / `fetch` calls whose URL string ends
in `/.well-known/openid-configuration` or `/introspect` and are *not* inside
a class-level constructor / module load.

**Severity**: **High** (introspection per request, discovery per request).

---

## 6. KMS / Envelope Encryption in Loops

### 6.1 `kms:Decrypt` per row

AWS KMS pricing: $0.03 per 10k requests; default rate limit 5,500–30,000
req/sec/region depending on key type. Loop pattern:
```python
for row in cursor:
    plaintext = kms.decrypt(CiphertextBlob=row.encrypted_field)['Plaintext']
```
1 M rows/day × 2 columns = $6/day = $180/mo per service, plus you'll
throttle. AWS docs explicitly claim "optimizing encryption patterns can
lower KMS costs by up to 99%."

### 6.2 Envelope encryption done wrong

The right shape:
1. `GenerateDataKey` → get plaintext DEK + encrypted DEK per object/batch/window.
2. Use DEK locally (AES-256-GCM) to encrypt many records.
3. Cache decrypted DEK in memory (AWS Encryption SDK has `CryptoMaterialsCache`).

Wrong shape: per-row `kms:Decrypt`. For a 100 MB file, "direct encryption"
requires 25,000+ KMS calls; envelope = 1.

### 6.3 Vault `secret/data/...` per request

Vault KVv1/KVv2 reads are **not leased** and Vault doesn't natively cache
them — that's why Vault Agent and Vault Proxy exist. Anti-pattern:
`vault.read("secret/data/db-creds")` inside a handler; right pattern: read
at startup, refresh on rotation signal, run Vault Agent sidecar with static
secret caching.

### Tree-sitter shape

```scheme
; AWS KMS decrypt inside for-loop
(for_statement
  body: (block
    (expression_statement
      (call function: (attribute
                        object: (identifier) @c (#eq? @c "kms")
                        attribute: (identifier) @m (#eq? @m "decrypt"))))))
```

**Severity**: **Critical** (KMS in loop — both perf and cost).
**Confidence**: high.

---

## 7. TLS / mTLS Patterns

### 7.1 HTTPS without keep-alive / HTTP/2

Each full TLS 1.3 handshake adds ≥100 ms; without keep-alive every request
pays. HTTP/2 multiplexing amortizes the handshake across many concurrent
requests on one connection. `requests.get()` per call (Python) creates a
new socket; should be `Session()`.

### 7.2 mTLS handshake per call

mTLS = TLS handshake + client certificate exchange + server validates client
cert vs CA. Even Envoy and NGINX explicitly document keepalive /
connection-pool configuration as the recommended mitigation. NGINX
certificate rotation forces a config reload; Envoy has hot reload — relevant
when deciding *where* to terminate mTLS.

### 7.3 OCSP stapling vs per-request validation

Without OCSP stapling, the server makes an OCSP responder call per cert
verify; stapling caches the OCSP response with the cert.

**Severity**: **High** (creating new HTTP client per request), **Medium**
(mTLS without keepalive).

---

## 8. CSRF / Rate-Limit / CORS Middleware Patterns

### 8.1 CSRF token regeneration per request

OWASP CSRF cheat sheet: "Tokens may be either regenerated on every
submission or kept the same throughout the life of the session… default
regeneration provides stricter security but may result in usability
concerns." Per-request rotation forces a server-side write per request (DB
or distributed cache hit). Per-session rotation is the right default unless
banking-tier protection is required. Double-submit cookie pattern is
stateless and eliminates server storage.

### 8.2 Rate limit hitting slow remote backend

A well-built Redis rate limiter using EVAL Lua scripts hits ~<1 ms P99 and
50k+ RPS, P95 <2 ms. Anti-patterns: using a SQL DB for rate-limit counters
(orders of magnitude slower; lock contention), making a network hop per
request to a remote rate-limit service without local cache.

### 8.3 CORS preflight not cached

`Access-Control-Max-Age` default = 5 seconds. Without it, polling clients
re-preflight every interval, "halving performance from the end user's
perspective." Browser caps: Firefox 86400 (24h), Chromium 7200 (2h). Set to
7200 explicitly. Cache is keyed by Method+URL, so query-string-varying URLs
blunt the cache.

**Severity**: **Medium** (CSRF rotate-per-request), **High** (rate-limit on
slow backend), **Medium** (missing `Access-Control-Max-Age`).

---

## 9. Authorization Framework Patterns

### 9.1 OPA without bundle caching

OPA "Policy Performance" docs: per-request budget on the order of 1 ms.
Bundles loaded into memory favor low latency. Bundle poll set too
aggressively (every minute) burns CPU + network; consolidation reduces this.
Goldman Sachs OCES post documents bundle consolidation as the fix for
activation latency. Batch queries (`POST /v1/data/path` with arrays) cut
per-decision overhead.

### 9.2 Casbin enforcer recreated per request

Casbin docs and issues #239, #682 explicitly: "recreating the enforcer per
request is not recommended." Anti-pattern: `e := casbin.NewEnforcer(...)`
inside HTTP handler. Right shape: singleton `SyncedEnforcer`, multiple
worker threads, RBAC over direct user perms for O(1) role cache. Issue #681
shows severe degradation with many policies when enforcer is rebuilt.

### 9.3 Cedar policy parse per call

Cedar (Rust) is engineered for "millisecond" evaluation, but per-call
`Policy::from_str()` re-parses — defeats the design. Build a `PolicySet`
once, reuse.

### 9.4 Spring Security `@PreAuthorize` SpEL

`SpEL` expressions are interpreted by default. `SpelCompilerMode.IMMEDIATE`
enables JIT compilation; without it, complex `@PreAuthorize` expressions
re-parse on every method invocation. WhozApp blog post: they abandoned
`@PreAuthorize` SpEL entirely for list authorizations due to "inefficiency
in handling list authorizations."

### 9.5 Oso

Oso Cloud claims <2 ms in-process, <10 ms end-to-end with Edge Nodes
precomputing indexes + caching queries. Anti-pattern: not deploying Edge
nodes in-region or building the policy object per request rather than at
startup.

**Severity**: **High** (enforcer/policy object built per request), **Medium**
(un-compiled SpEL, OPA bundle re-fetch). **Confidence**: medium — context
dependent.

---

## 10. Logging / Audit-Trail Performance

### 10.1 Sync audit-log DB write per request

Documented anywhere from 30–40% application throughput degradation when
audit writes block the request. Right shape: 202 Accepted + Kafka/queue +
background consumer; or buffer 500 ms / batch 100–500 events and bulk-insert.

### 10.2 Audit log on hot read path

Reads usually shouldn't audit. If compliance requires it, append-only event
log to a separate Kafka topic with batching, not a multi-table transactional
write.

### 10.3 PII redaction regex compiled per log call

`re.compile(pattern)` inside a log filter that runs per record =
significant overhead. Confirmed: 5–8% CPU at 10k logs/sec with 6 patterns
*when compiled once*; uncompiled is multiples of that. Compile module-level;
order patterns most-common first.

### 10.4 `logging.getLogger(__name__)` in hot loops

Python's `getLogger` is internally cached, so cost is low — but allocating
loggers with dynamic names per call defeats the cache. Get the logger once
at module scope.

### Tree-sitter shape

```scheme
; re.compile inside function body (should be module-level)
(function_definition
  body: (block
    (expression_statement
      (call function: (attribute
                        object: (identifier) @lib (#eq? @lib "re")
                        attribute: (identifier) @m (#eq? @m "compile"))))))
```

**Severity**: **High** (sync audit write in handler), **Medium** (regex
compile in loop). **Confidence**: high.

---

## 11. Existing Analyzers — What They Do, What Drift Should Add

Quick map of overlap so drift-static-profiler covers gaps, not duplicates.

| Tool | Language | License | Perf-adjacent rules | Notable |
|---|---|---|---|---|
| **Bandit** | Python | Apache-2.0 | B324 (insecure hashlib MD5/SHA1), B301 (pickle), B311 (`random` for security) | ~90 plugin rules; YAML-configurable. No explicit perf-cost rules. https://github.com/PyCQA/bandit |
| **gosec** | Go | Apache-2.0 | G401–G404 (weak crypto/PRNG), G505 (DES/RC4/MD5) — security framing but `crypto/rand` is slower per call. https://github.com/securego/gosec | G404 detects `math/rand` for security — flip side: crypto/rand is slower API |
| **SpotBugs + FindSecBugs** | Java | LGPL-3.0 | `STATIC_IV`, `ECB_MODE`, `PADDING_ORACLE`, `CIPHER_INTEGRITY`, `HARD_CODE_PASSWORD`, `WEAK_MESSAGE_DIGEST_MD5/SHA1`, `DES_USAGE`. 144 vulnerability types, 826 API signatures. https://github.com/find-sec-bugs/find-sec-bugs | Has cipher detectors but no "Cipher created in loop" perf rule. |
| **Brakeman** | Ruby/Rails | MIT | 33 vulnerability categories — confidence levels high/medium/weak. No explicit perf-cost rules. https://brakemanscanner.org/ |
| **eslint-plugin-security** | JS/Node | Apache-2.0 | `detect-non-literal-regexp`, `detect-unsafe-regex` (ReDoS), `detect-non-literal-require`, `detect-non-literal-fs-filename`. https://github.com/eslint-community/eslint-plugin-security | ReDoS rules ARE perf-adjacent. |
| **Semgrep** | Polyglot | LGPL-2.1 | `p/jwt`, `p/secrets`, `p/owasp-top-ten`, ~38 secret-detection rules. https://semgrep.dev/ | Pattern-based — easy to author custom perf rules. |
| **SonarQube Security Hotspots** | Polyglot | Commercial + community | Hotspots ranked by OWASP Top 10 + CWE Top 25 review priority. Hotspots are review-prompts, not perf rules. https://docs.sonarsource.com/sonarqube-server/user-guide/security-hotspots/ |
| **PMD** | Java/Apex | BSD | Open issue #3222 — `Reuse / share SecureRandom instance` perf rule (not yet merged as of search). https://github.com/pmd/pmd/issues/3222 |

**Gaps Drift can fill (none of the above cover these directly):**
- "Hashing/crypto call inside a for-loop body"
- "KMS / Vault / Redis-secret call inside a for-loop"
- "JWKS fetch inside verify function (not at module init)"
- "Casbin/OPA/Cedar policy object constructed inside handler"
- "Sync bcrypt/argon2 inside async function body"
- "RSA key generation anywhere outside a CLI / init script"
- "`requests.get` / `httpx.get` without a shared `Session()` for a hostname
   seen ≥10× in module"
- "regex `re.compile` inside function body"
- "audit-log DB write call on a read endpoint"

These are exactly the patterns that need cross-file dataflow + categorical
context, which the existing categories (Db/Network/Io/Cache/Queue/Log)
already provide — the new `Crypto` / `Auth` category in
`src/categories.rs` lets you reuse the in-loop / in-handler detectors
already wired into `graph.rs`.

---

## Suggested Drift Category Additions

Add to `categories.rs` alongside existing ones:
- `Crypto` — bcrypt/argon2/pbkdf2/hashlib/hmac/RSA/ECDSA/Ed25519/AES/cipher
- `Auth` — JWT, JWKS, OIDC, session, OAuth, SAML, CSRF
- `Authz` — OPA, Casbin, Cedar, Oso, Spring `@PreAuthorize`, Django Guardian
- `Secrets` — KMS, Vault, Secrets Manager, GCP KMS, Azure Key Vault
- `TLS` — TLS handshake, cert parsing, OCSP, mTLS

Wire each into the existing in-loop / in-handler / per-request-context
detection in `graph.rs` so the rules compose: "Crypto in Loop" reuses the
loop detector already used by ORM N+1; "Auth without Cache" reuses the
Cache-category absence check; "Secrets in Loop" reuses the same loop
detector as KMS specifically. Severity defaults: Critical for
RSA-keygen-in-handler and KMS-in-loop, High for sync-hash-in-async and
JWKS-per-request, Medium for regex-compile-in-loop and missing CORS max-age.

---

## Sources

**Password hashing**
- [Password Storage — OWASP Cheat Sheet Series](https://cheatsheetseries.owasp.org/cheatsheets/Password_Storage_Cheat_Sheet.html)
- [bcrypt Benchmark — patrickfav/bcrypt wiki](https://github.com/patrickfav/bcrypt/wiki/Benchmark)
- [PHP 8.4 — Default bcrypt cost increased to 12](https://php.watch/versions/8.4/password_hash-bcrypt-cost-increase)
- [Argon2-cffi — Choosing Parameters](https://argon2-cffi.readthedocs.io/en/stable/parameters.html)
- [RFC 9106 — Argon2](https://datatracker.ietf.org/doc/html/rfc9106)
- [PBKDF2 iteration counts (DEV)](https://dev.to/securebitchat/why-you-should-use-310000-iterations-with-pbkdf2-in-2025-3o1e)
- [bcrypt blocking the event loop in Node](https://medium.com/lets-code-future/bcrypt-was-blocking-our-event-loop-heres-what-we-used-instead-98c93b75190f)

**JWT / JWKS / OIDC**
- [WorkOS Developer's Guide to JWKS](https://workos.com/blog/developers-guide-jwks)
- [auth0/node-jwks-rsa examples](https://github.com/auth0/node-jwks-rsa/blob/master/EXAMPLES.md)
- [Nimbus algorithm selection guide](https://connect2id.com/products/nimbus-jose-jwt/algorithm-selection-guide)
- [JWT Framework benchmark table](https://web-token.spomky-labs.com/benchmark/result-table)
- [WorkOS JWT validation guide](https://workos.com/guide/jwt-validation)
- [HMAC vs RSA vs ECDSA for JWTs](https://workos.com/blog/hmac-vs-rsa-vs-ecdsa-which-algorithm-should-you-use-to-sign-jwts)
- [OAuth Token Introspection](https://www.oauth.com/oauth2-servers/token-introspection-endpoint/)
- [Keycloak OIDC docs](https://www.keycloak.org/securing-apps/oidc-layers)

**Crypto in loops / TLS**
- [Java Random vs SecureRandom](https://medium.com/javarevisited/java-random-vs-securerandom-stop-using-the-wrong-one-3ed573a39fd4)
- [PMD issue #3222 — Reuse SecureRandom rule](https://github.com/pmd/pmd/issues/3222)
- [Python hashlib docs](https://docs.python.org/3/library/hashlib.html)
- [TLS Handshake Latency](https://www.systemoverflow.com/learn/networking-protocols/http-protocols/tls-handshake-latency-the-critical-path-tax-across-protocol-versions)
- [Stop Paying for the Same TLS Handshake Twice](https://medium.com/@adarshpandey.pandey355/stop-paying-for-the-same-handshake-twice-how-reusing-tls-connections-slashes-infrastructure-costs-d032b8e4f132)
- [Teleport — Ditching RSA for 77% CPU reduction](https://goteleport.com/blog/ditching-rsa-made-teleport-more-efficient/)
- [Envoy reverse tunnels (mTLS reuse)](https://www.envoyproxy.io/docs/envoy/latest/configuration/other_features/reverse_tunnel)

**Sessions / cookies**
- [Cache vs Session Store — Redis blog](https://redis.io/blog/cache-vs-session-store/)
- [Logto — Cookie size exceeded / split](https://blog.logto.io/cookie-size-exceeded)
- [Paul Calvano — Cookie size analysis](https://paulcalvano.com/2020-07-13-an-analysis-of-cookie-sizes-on-the-web/)

**KMS / Vault**
- [Optimize AWS KMS decryption costs — AWS blog](https://aws.amazon.com/blogs/database/optimize-aws-kms-decryption-costs-for-database-activity-streams/)
- [Envelope Encryption on AWS KMS](https://wolfman.dev/posts/aws-kms-for-envelope-encryption/)
- [Vault Agent caching](https://developer.hashicorp.com/vault/docs/agent-and-proxy/agent/caching)
- [Vault static secret caching (Proxy)](https://developer.hashicorp.com/vault/docs/agent-and-proxy/proxy/caching/static-secret-caching)

**CSRF / CORS / Rate limit**
- [OWASP CSRF Prevention Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/Cross-Site_Request_Forgery_Prevention_Cheat_Sheet.html)
- [HttpToolkit — Cache your CORS for performance & profit](https://httptoolkit.com/blog/cache-your-cors/)
- [Optimizing CORS preflight](https://webperf.tips/tip/optimizing-cors/)
- [Token Bucket with Redis — Redis docs](https://redis.io/docs/latest/develop/use-cases/rate-limiter/)

**Authorization frameworks**
- [OPA Policy Performance docs](https://www.openpolicyagent.org/docs/policy-performance)
- [Casbin Performance Optimization](https://casbin.org/docs/performance/)
- [Casbin issue #239 — best practices](https://github.com/casbin/Casbin.NET/issues/239)
- [Cedar Policy Language](https://docs.cedarpolicy.com/)
- [Oso Authorization Query Performance](https://www.osohq.com/docs/oso-in-depth/authorization-query-performance)
- [Spring Security Expression-Based Access Control](https://docs.spring.io/spring-security/reference/servlet/authorization/method-security.html)
- [Why we left Spring Security SpEL behind](https://medium.com/whozapp/why-we-left-spring-security-spel-behind-1325d88c1133)

**Logging / audit**
- [Elastic Observability — PII detection in logs](https://www.elastic.co/observability-labs/blog/pii-ner-regex-assess-redact-part-1)
- [Audit logs with CDC and Kafka — Debezium](https://debezium.io/blog/2019/10/01/audit-logs-with-change-data-capture-and-stream-processing/)
- [Confluent — Real-time compliance/audit logging with Kafka](https://www.confluent.io/blog/build-real-time-compliance-audit-logging-kafka/)

**Existing analyzers**
- [Bandit — GitHub](https://github.com/PyCQA/bandit)
- [gosec — GitHub](https://github.com/securego/gosec)
- [Find Security Bugs — GitHub](https://github.com/find-sec-bugs/find-sec-bugs)
- [Brakeman scanner](https://brakemanscanner.org/)
- [eslint-plugin-security](https://github.com/eslint-community/eslint-plugin-security)
- [eslint-plugin-security — detect-unsafe-regex](https://github.com/eslint-community/eslint-plugin-security/blob/main/docs/rules/detect-unsafe-regex.md)
- [Semgrep JWT ruleset](https://semgrep.dev/p/jwt)
- [SonarQube Security Hotspots docs](https://docs.sonarsource.com/sonarqube-server/user-guide/security-hotspots/)

**Tree-sitter**
- [py-tree-sitter Query class](https://tree-sitter.github.io/py-tree-sitter/classes/tree_sitter.Query.html)
- [Tree-sitter query syntax basics](https://tree-sitter.github.io/tree-sitter/using-parsers/queries/1-syntax.html)
