I have ample material now. Let me write up the deep research report directly as the assistant output (per instructions: do NOT write a .md file, return findings directly).

---

# Deep Research: AI/ML/LLM Performance Anti-Patterns for Static Detection in `drift-static-profiler`

Audience: drift-static-profiler rule-catalog authors. Same rule shape as the existing ORM catalogs — every entry below names an import set, a receiver/call signature for matching, a loop-body shape, and a counter-pattern (the fix). Detection tiers map to the existing `ClassifyTier` enum in `src/categories.rs`:

- **Tier B (`ImportedModule`)** = high-precision: receiver bound to a catalogued import. Pure tree-sitter, near-zero false positives.
- **Tier C (`ReceiverPattern`)** = receiver-name heuristic (`openai_client`, `embed_model`, `index`, etc.).
- **Tier D (`MethodSignature`)** = method-name-only, weak unless combined with loop-body shape.
- **Runtime-only** = needs trace data (request count, latency p95). Drift cannot catch these statically; flag them in `INSIGHTS_PLAN.md` for the runtime sibling.

A new `Category::Llm` (or two: `Llm`, `Vector`) is implied throughout — the existing `Compute` bucket is too coarse, and these calls are simultaneously network-bound and cost-bound, which the cost-estimator pass (section 8) wants to surface distinctly.

---

## 1. LLM API Call Patterns

### 1.1 Per-call SDK client construction (the HttpClient bug, again)

This is the textbook drift case ported to AI. The OpenAI, Anthropic, Cohere, Mistral, Google Gen AI, Groq, Together, Fireworks, and Replicate Python SDKs all wrap an internal `httpx.Client` / `httpx.AsyncClient` with a connection pool. Constructing a fresh `OpenAI()` per request defeats keep-alive, defeats the pool, and adds tail latency. The Groq Python SDK explicitly documents that the client is "powered by httpx" and supplies both sync and async clients precisely so they can be reused ([groq/groq-python](https://github.com/groq/groq-python)). DeepWiki's openai-python page and the OpenAI Agents SDK config docs both call out client reuse as the recommended pattern ([Client Configuration | DeepWiki](https://deepwiki.com/openai/openai-python/2.1-client-configuration), [OpenAI Agents SDK config](https://openai.github.io/openai-agents-python/config/)).

- **Import set**: `from openai import OpenAI, AsyncOpenAI` / `from anthropic import Anthropic, AsyncAnthropic` / `from cohere import Client, AsyncClient` / `from mistralai import Mistral` / `from google import genai` / `from groq import Groq, AsyncGroq` / `from together import Together` / `import replicate` / `from fireworks.client import Fireworks`.
- **AST shape (Tier B)**: a `call_expression` whose callee resolves to one of the above class names, whose enclosing scope is a function decorated with `@app.{get,post,route}`, `@router.{get,post}`, `async def` of a FastAPI handler, a Flask view function, or a Django view. The same rule shape you already use for `requests.Session()` per-handler.
- **Loop-body shape**: same constructor inside a `for`/`while` body.
- **Counter-pattern**: module-level binding (`client = OpenAI()`), `lru_cache`d factory, FastAPI `Depends(get_client)`, or framework lifespan hook.
- **Detector tier**: Tier B (high precision).

### 1.2 Missing Anthropic prompt caching (`cache_control`)

Anthropic's caching requires explicit `"cache_control": {"type": "ephemeral"}` markers on system/tools/messages content blocks. The minimum cached block is 1024 tokens; up to 4 breakpoints per prompt; correct placement is on the *last* tool definition so the prefix `tools + system` is one cache block ([Prompt caching — Claude API Docs](https://platform.claude.com/docs/en/build-with-claude/prompt-caching), [anthropics/anthropic-cookbook prompt_caching.ipynb](https://github.com/anthropics/anthropic-cookbook/blob/main/misc/prompt_caching.ipynb)). The "dynamic content in the cached block" anti-pattern (timestamp injected into the system prompt) silently breaks caching — every call is a cache miss ([dev.to: I Was Caching Wrong This Whole Time](https://dev.to/yurukusa/i-was-caching-wrong-this-whole-time-anthropic-academy-part-1-1hba)).

- **Tier B shape**: `<client>.messages.create(...)` (or `.beta.messages.create`) where the import set contains `from anthropic import` AND no kwarg dict literal in `system=` / `messages=` contains the string `cache_control`. Combine with a length heuristic on adjacent string literals — if the `system=` string-literal is > ~2000 chars (proxy for 1024 tokens), it is a likely cache miss. Detector emits *opportunity*, not *bug*.
- **Anti-pattern shape**: a string concatenation `f"... {datetime.now()} ..."` or `f"... {request_id} ..."` *inside* a `messages=` / `system=` argument where any other content block has `cache_control` — the prefix is invalidated each call.
- **Tier**: Tier B for "no cache_control anywhere"; Tier C for the "dynamic-content-poisons-prefix" variant.

### 1.3 Missing OpenAI prompt-prefix caching

OpenAI's caching is *automatic* on prompts > 1024 tokens, but only when the prefix is *byte-stable*. The same "stable prefix" rule applies: PromptHub's comparison and DigitalOcean's prompt-caching writeup both list "putting dynamic values at the top of the prompt" as the dominant cause of zero hit-rate ([PromptHub: Prompt Caching with OpenAI, Anthropic, and Google](https://www.prompthub.us/blog/prompt-caching-with-openai-anthropic-and-google-models), [DigitalOcean: Prompt Caching](https://www.digitalocean.com/blog/prompt-caching-with-digital-ocean)).

- **Tier C shape**: `messages` array where the *last* message (user) is a static literal and the *first* message (system) contains an f-string interpolation. That's an inverted-prefix smell. Hard to detect cleanly — leave to Tier 2 heuristic.

### 1.4 Sync calls in async handlers

`openai.OpenAI().chat.completions.create(...)` inside an `async def` route blocks the event loop. The fix is `AsyncOpenAI` and `await`. This is now the headline FastAPI+LLM bug in production — multiple writeups describe how a single sync call serialises 100 concurrent requests behind a 2-second OpenAI call ([The Concurrency Mistake Hiding in Every FastAPI AI Service](https://jamwithai.substack.com/p/the-concurrency-mistake-hiding-in), [Async/Await Isn't Enough](https://medium.com/@aryanrot234/async-await-isnt-enough-solving-synchronous-bottlenecks-in-fastapi-6f9f152256a2)).

- **Tier B shape**: receiver imported from `openai` (the sync class `OpenAI`, not `AsyncOpenAI`) called inside an `async def` whose enclosing module imports FastAPI/Starlette/aiohttp/Quart. Same rule for `Anthropic` vs `AsyncAnthropic`, `Cohere.Client` vs `AsyncClient`, `Groq` vs `AsyncGroq`.
- **Counter-pattern**: `AsyncOpenAI` + `await`, or sync client wrapped in `asyncio.to_thread(...)`.
- **Tier**: Tier B — extremely high precision once the async-class taxonomy is hardcoded.

### 1.5 LLM call inside a `for` loop (the LLM-N+1)

The single most expensive shape we can detect. OpenAI, Anthropic, Mistral, and Cohere all expose a Batch API: 24h SLA, ~50 % discount, no rate-limit pressure. Groq also has `/v1/batches` ([Groq Batch API](https://console.groq.com/docs/batch)). The drift ORM N+1 detector ports almost verbatim.

- **Tier B shape**: `for x in <iterable>: ... <client>.{chat,messages,responses}.completions.create(...)` where the client is bound to one of the catalogued LLM SDK imports.
- **Counter-pattern**: presence of `client.batches.create(...)` (OpenAI), `client.messages.batches.create(...)` (Anthropic), or `asyncio.gather(*[...])` in surrounding scope.
- **Tier**: Tier B. Emit `category=llm, severity=high, fix=batch_api`.

### 1.6 Missing streaming on long completions

Affects UX (TTFB) and risks HTTP timeouts. Anthropic explicitly requires `stream=True` for large `max_tokens` to avoid HTTP timeouts ([Anthropic Streaming messages](https://docs.anthropic.com/en/docs/build-with-claude/streaming)).

- **Tier C shape**: `.create(..., max_tokens=N, ...)` with `N` literal > 4096 and no `stream=True`.
- **Tier**: Tier C.

### 1.7 Missing retries with exponential backoff

OpenAI's official cookbook recommends `tenacity.@retry(wait=wait_random_exponential(min=1, max=60), stop=stop_after_attempt(6))` ([How to handle rate limits — OpenAI Cookbook](https://cookbook.openai.com/examples/how_to_handle_rate_limits), [openai/openai-cookbook How_to_handle_rate_limits.ipynb](https://github.com/openai/openai-cookbook/blob/main/examples/How_to_handle_rate_limits.ipynb)). The OpenAI/Anthropic SDKs already do up to 2 retries internally; the anti-pattern is `max_retries=0` or a hand-rolled `try/except` that just re-raises.

- **Tier B shape**: any call to a catalogued LLM client where the SDK was constructed with `max_retries=0`, OR the enclosing function lacks a `@retry` decorator and the call is not inside a `try/except RateLimitError`. Weak signal alone; emit as Tier-3 advisory.

### 1.8 Tokenizer load inside the request handler

`tiktoken.get_encoding("cl100k_base")` or `tiktoken.encoding_for_model("gpt-4o")` triggers a one-time vocabulary download (cached in `~/.tiktoken`) and a parse. Reuse is the documented best practice — "load your encoding once at application startup and reuse that same encoder object" ([tiktoken caching strategy — DeepWiki](https://deepwiki.com/openai/tiktoken/5.1-caching-strategy-and-configuration), [openai/tiktoken](https://github.com/openai/tiktoken)). Same story for `transformers.AutoTokenizer.from_pretrained` and `sentencepiece.SentencePieceProcessor`.

- **Tier B shape**: `tiktoken.get_encoding(...)`, `tiktoken.encoding_for_model(...)`, `AutoTokenizer.from_pretrained(...)`, `SentencePieceProcessor()` called inside a function annotated as a request handler, or inside a `for` loop.
- **Counter-pattern**: module-level binding, `@functools.cache`, FastAPI `lifespan`.
- **Tier**: Tier B.

### 1.9 Embeddings: one-at-a-time vs batched (max 2048)

OpenAI embeddings accept up to **2048 inputs per request** and a hard cap of **300 000 tokens summed across inputs** (each input ≤ 8192 tokens) ([Embeddings API Max Batch Size — OpenAI Community](https://community.openai.com/t/embeddings-api-max-batch-size/655329), [OpenAI Embeddings API reference](https://platform.openai.com/docs/api-reference/embeddings/create)). Sending one text at a time inside a loop is the embedding-version of the LLM-N+1.

- **Tier B shape**: `for text in texts: <client>.embeddings.create(input=text, model=...)` where the client is bound to `openai`, `voyageai`, `cohere` (`co.embed`), or `mistralai` (`client.embeddings.create`).
- **Counter-pattern**: passing a list literal or list comprehension to `input=`.
- **Tier**: Tier B.

### 1.10 Missing `max_tokens`

Anthropic *requires* `max_tokens`; OpenAI does not but unbounded outputs blow cost budgets. The pydantic-ai project even ran into the issue of model max defaults timing out non-streaming requests, hence their forced 4096 default ([pydantic-ai issue #2553](https://github.com/pydantic/pydantic-ai/issues/2553)).

- **Tier B shape**: `.create(...)` call missing the `max_tokens` (Anthropic / OpenAI Responses) or `max_completion_tokens` (OpenAI Chat Completions, o1+) kwarg, where the call is inside a user-input flow (a handler that takes a string from a `Request` body). Easy false positive on local CLI scripts — gate on "is in a server context".
- **Tier**: Tier B for Anthropic (it would be a runtime error if unset — so a missed-kwarg detector with very low FP rate); Tier C for OpenAI.

### 1.11 JSON-mode / tool-use without schema validation

OpenAI's classic `response_format={"type": "json_object"}` does not validate; only `response_format={"type": "json_schema", "json_schema": {..., "strict": True}}` is schema-bound. Critically: **Structured Outputs is incompatible with `parallel_tool_calls=True`** — you must set `parallel_tool_calls=False` or supplied schemas may not match ([Introducing Structured Outputs in the API — OpenAI](https://openai.com/index/introducing-structured-outputs-in-the-api/), [Structured model outputs guide](https://developers.openai.com/api/docs/guides/structured-outputs)).

- **Tier B shape (a)**: `response_format={"type": "json_object"}` without a `json.loads(...)` wrapped in `try/except` downstream — flag as "unvalidated JSON".
- **Tier B shape (b)**: `response_format={"type": "json_schema", ...}` AND `parallel_tool_calls=True` in the same call — this is a correctness bug, not perf, but it's a free catch.
- **Tier**: Tier B.

### 1.12 Hard-coded model names

Operationally, the call site `model="gpt-4o"` baked into source is hostile to cost-tier rollouts (cf. internal dev/staging routing to a cheaper model). Worth surfacing as a *catalog* finding, not an error.

- **Tier C shape**: any string literal in `model=` that matches the model regex `^(gpt-|claude-|gemini-|mistral-|llama-|command-)`. Counter-pattern is `os.getenv(...)` or a settings object.

### 1.13 Repeated identical prompts without memoization

If a prompt is deterministic in its inputs (no `random`/`now`), `@functools.cache` or a Redis read-through is dramatically cheaper than even Anthropic's prefix cache (zero token cost) ([IBM: What is Prompt Caching?](https://www.ibm.com/think/topics/prompt-caching)).

- **Tier C shape**: a function whose body is a single LLM call and whose parameters are hashable primitives — recommend `@functools.lru_cache`. False-positive prone (non-deterministic, side-effecting). Tier-2 heuristic only.

---

## 2. Embedding Workflows

### 2.1 Model load per call (sentence-transformers / fastembed)

`SentenceTransformer("all-MiniLM-L6-v2")` downloads weights from HF Hub on first construct and loads ~80MB into RAM. Inside a handler this is catastrophic. `fastembed.TextEmbedding(...)` and `voyageai.Client()` follow the same pattern.

- **Tier B shape**: `SentenceTransformer(...)`, `CrossEncoder(...)`, `fastembed.TextEmbedding(...)`, `OpenAIEmbeddings(...)` (LangChain), `HuggingFaceEmbeddings(...)` (LangChain) inside a function flagged as a handler. Import set: `from sentence_transformers import` / `from fastembed import` / `from langchain_openai import OpenAIEmbeddings` / `from langchain_huggingface import HuggingFaceEmbeddings`.
- **Tier**: Tier B.

### 2.2 `.encode([text])` per row vs `.encode([many])` batched

sentence-transformers' default `batch_size=32` is documented as inefficient on modern GPUs — encode the whole list with a tuned `batch_size` ([Medium: Correctly Set Batch Size in Sentence Transformers](https://medium.com/@vici0549/it-is-crucial-to-properly-set-the-batch-size-when-using-sentence-transformers-for-embedding-models-3d41a3f8b649), [sentence-transformers Issue #2551](https://github.com/huggingface/sentence-transformers/issues/2551)). One important wrinkle: encoding the same texts with a different batch size produces (very slightly) different embeddings, so changing batching can invalidate stored vectors ([Issue #2312](https://github.com/UKPLab/sentence-transformers/issues/2312)).

- **Tier B shape**: `for x in xs: model.encode([x])` or `model.encode(x)` (single-string) inside a `for` over an iterable where the loop body doesn't mutate `xs`.
- **Counter-pattern**: `model.encode(xs, batch_size=128)`.
- **Tier**: Tier B.

### 2.3 Missing CUDA/MPS device placement

`SentenceTransformer("all-MiniLM-L6-v2")` defaults to CPU if `device=` is not passed and `torch.cuda.is_available()` is False at import time. Forgetting `device="cuda"` on a GPU box silently runs on CPU.

- **Tier C shape**: `SentenceTransformer(name)` with no `device=` kwarg and no `model.to("cuda"|"mps")` follow-up in scope. Low precision — surface as advisory.

### 2.4 fp16/int8 quantization not used

- **Tier C shape**: `SentenceTransformer(...)` without `model_kwargs={"torch_dtype": torch.float16}` or `precision="int8"` on `.encode`. Advisory; only emit when GPU import is present.

---

## 3. Vector Search SDKs (Pinecone, Qdrant, Weaviate, Milvus, Chroma, pgvector, FAISS, USearch, LanceDB)

### 3.1 `upsert([one])` in loop

Every vector DB exposes a bulk upsert. The list of method names is short and very specific — a perfect Tier-B port of the existing ORM bulk-write detector.

- **Imports / call shapes**:
  - Pinecone: `from pinecone import Pinecone` → `index.upsert(vectors=[...])`. Batch up to 100 vectors per request recommended ([Pinecone Docs](https://docs.pinecone.io/integrations/llamaindex)).
  - Qdrant: `from qdrant_client import QdrantClient` → `client.upsert(collection_name, points=[...])` or `client.upload_collection(...)`.
  - Weaviate: `from weaviate import` → `client.batch.add_object(...)` inside `with client.batch as b:` context — *not* using the batch context is the anti-pattern.
  - Milvus / pymilvus: `collection.insert([...])` — supports lists.
  - Chroma: `collection.add(ids=[...], embeddings=[...])`.
  - pgvector: any `INSERT INTO ... VALUES ($1)` in a loop where the parameter type is `vector` — drift already has the ORM N+1 detector for this; the new rule just tags the category as `vector`.
  - FAISS: `index.add(np.array([single]))` in a loop.
- **Tier B shape**: any of the above method calls inside a `for` body, with no `executemany`/list-literal/`.batch` context in scope.
- **Tier**: Tier B.

### 3.2 Filter pre vs post (different cost models)

Qdrant supports payload filters during HNSW traversal (pre-filter); Pinecone exposes `filter=` which is a hybrid pre/post (pod-based: post, serverless: pre); Weaviate does pre-filter through the inverted index. Doing filtering *after* the fetch (`results = client.query(...); [r for r in results if r.payload["lang"] == "en"]`) means the top_k was wasted.

- **Tier C shape**: `<index>.query(top_k=K, ...)` immediately followed by a Python `[r for r in results if ...]` or `filter(...)` call against the result list. Counter-pattern: pushdown into the SDK's native `filter=` argument.
- **Tier**: Tier C.

### 3.3 `top_k` too high

- **Tier D shape**: `top_k=` literal > 100. Single-rule.

### 3.4 Missing index (brute-force ANN)

FAISS' `IndexFlatL2` / `IndexFlatIP` is exact brute-force; useful below ~1M vectors but wasteful above ([FAISS brute-force search wiki](https://github.com/facebookresearch/faiss/wiki/Brute-force-search-without-an-index)). USearch's `exact=True` is the analogous footgun ([USearch HN thread](https://news.ycombinator.com/item?id=36942993)). pgvector with no `CREATE INDEX ... USING ivfflat|hnsw` falls back to a sequential scan.

- **Tier B shape**: `faiss.IndexFlatL2(d)` or `faiss.IndexFlatIP(d)` constructed at module scope. Emit advisory; can't statically know dataset size.

### 3.5 Wrong distance metric (cosine vs L2 vs IP)

The dominant pgvector anti-pattern: **the operator used in `ORDER BY` must match the operator class used at `CREATE INDEX`**, or the index is bypassed silently ([AWS: pgvector indexing deep dive](https://aws.amazon.com/blogs/database/optimize-generative-ai-applications-with-pgvector-indexing-a-deep-dive-into-ivfflat-and-hnsw-techniques/), [pgvector creating and using indexes — DeepWiki](https://deepwiki.com/pgvector/pgvector/5.2-creating-and-using-indexes)). Operators: `<->` L2, `<#>` IP, `<=>` cosine, `<+>` L1.

- **Tier B shape (SQL)**: a `CREATE INDEX ... USING hnsw (col vector_l2_ops)` migration combined with query string `ORDER BY col <=> $1` — operator mismatch. Drift's `parser.rs` would need a SQL-string scanner. Tier-2 from pure tree-sitter Python — easier in the SQL migration scan.

### 3.6 Embedding dim mismatch caught at insert vs search

pgvector limits index to 2000 dims (halfvec for higher) ([pgvector creating indexes](https://deepwiki.com/pgvector/pgvector/5.2-creating-and-using-indexes)). Pinecone/Qdrant raise on insert. Tier-3 (runtime).

### 3.7 Reranker call in loop

Cohere's `co.rerank(query, documents=[...], top_n=N)` accepts up to 1000 documents per call ([Cohere Rerank API](https://docs.cohere.com/reference/rerank)). Per-document reranking in a loop is the same N+1 shape as embeddings.

- **Tier B shape**: `for doc in docs: co.rerank(query=q, documents=[doc])`.

---

## 4. LangChain / LlamaIndex / Haystack / DSPy / Semantic Kernel

### 4.1 LLM call in a `for` (the obvious one)

- **Tier B shape**: `for x in xs: chain.invoke({...})` where `chain` is a `RunnableSequence` / `LLMChain` / DSPy `Predict`/`ChainOfThought`. Counter-pattern: `chain.batch([{...}, ...])` or `chain.abatch(...)`.

### 4.2 LangChain deprecated `LLMChain`

`LLMChain` is deprecated since 0.1.17, scheduled for removal in 0.3.0; the replacement is LCEL `prompt | llm` returning a `RunnableSequence` ([LangChain v0.2 deprecations](https://python.langchain.com/v0.2/docs/versions/v0_2/deprecations/), [LangChain LLMChain API](https://api.python.langchain.com/en/latest/chains/langchain.chains.llm.LLMChain.html)).

- **Tier B shape**: `from langchain.chains import LLMChain` → `LLMChain(llm=..., prompt=...)`. Emit fix: `prompt | llm` LCEL.

### 4.3 LCEL chain iterated without `.batch()`

`RunnableSequence` has both sync `batch()` and `abatch()` for parallel processing ([LangChain RunnableSequence](https://api.python.langchain.com/en/latest/runnables/langchain_core.runnables.base.RunnableSequence.html), [LCEL explained — Pinecone](https://www.pinecone.io/learn/series/langchain/langchain-expression-language/)).

- **Tier B shape**: `for x in xs: chain.invoke(x)` where chain's type can be inferred as `RunnableSequence`. Counter-pattern: `chain.batch(xs)`.

### 4.4 Tool calls in agents without `parallel_tool_calls`

OpenAI agents and LangChain `create_agent` / `create_react_agent` default to parallel tool calls when the model supports them; explicitly setting `parallel_tool_calls=False` (or not opting in for compatible models) serialises tool execution unnecessarily ([Why Parallel Tool Calling Matters](https://www.codeant.ai/blogs/parallel-tool-calling), [JS LangChain: How to disable parallel tool calling](https://js.langchain.com/docs/how_to/tool_calling_parallel/)). LLMCompiler benchmarks show up to 3.7× speedups.

- **Tier C shape**: `bind_tools(..., parallel_tool_calls=False)` — flag as performance miss unless paired with Structured Outputs (then it's required; see §1.11).

### 4.5 `RetrievalQA` / `VectorStoreIndex.from_documents` per request

Index construction belongs at startup, not in the handler. LlamaIndex docs explicitly call out `StorageContext.from_defaults(persist_dir=...)` + `load_index_from_storage()` as the production pattern ([LlamaIndex Persisting & Loading Data](https://docs.llamaindex.ai/en/stable/module_guides/storing/save_load)).

- **Tier B shape**: `VectorStoreIndex.from_documents(...)`, `VectorStoreIndex(...)`, `SimpleDirectoryReader(...).load_data()`, `RetrievalQA.from_chain_type(...)`, `Haystack Pipeline()` constructor, `dspy.Retrieve(...)` constructor — all inside a function flagged as a handler.
- **Counter-pattern**: module-level binding, `load_index_from_storage`.
- **Tier**: Tier B.

### 4.6 Memory mutated across requests (state leak)

`ConversationBufferMemory` bound at module scope is shared across users — security/correctness bug as well as a perf one (unbounded growth).

- **Tier C shape**: module-level `ConversationBufferMemory()` referenced inside a handler.

### 4.7 Streaming generator consumed eagerly with `list(stream)`

Defeats streaming entirely.

- **Tier B shape**: `list(<x>)` where `<x>` is the return of a method whose name is `.stream(...)` / `.astream(...)` / `with_streaming(...)`, or whose call has `stream=True` kwarg. Same for `for _ in stream: pass` followed by `result = full_response_var` reconstruction — that's a manual eager fold.

### 4.8 Haystack / Semantic Kernel pipelines rebuilt per request

`Pipeline()` in Haystack and `Kernel()` in Semantic Kernel are heavy objects with embedded retrievers. Same shape as 4.5.

- **Tier B shape**: `Pipeline()`, `kernel = Kernel()`, `kernel.add_service(...)` inside a handler body. Imports: `from haystack import Pipeline`, `from semantic_kernel import Kernel`.

---

## 5. Python ML Stack (PyTorch / TensorFlow / JAX / sklearn / numpy / pandas / polars)

### 5.1 `.cpu()` / `.numpy()` / `.item()` per tensor in loop

`.item()` and `.cpu()` are explicit `cudaStreamSynchronize` points — the canonical PyTorch perf bug ([PyTorch Forums: Tensor.item() takes a lot of running time](https://discuss.pytorch.org/t/tensor-item-takes-a-lot-of-running-time/16683/21), [Unnecessary cuda synchronizations Issue #108968](https://github.com/pytorch/pytorch/issues/108968), [PyTorch performance optimization](https://discuss.pytorch.org/t/performance-optimization-re-cpu-gpu-synchronization/157251)).

- **Tier B shape**: any of `.item()`, `.cpu()`, `.numpy()`, `.tolist()` on the result of a method whose receiver is inferable as a `torch.Tensor` (heuristic: receiver was returned from `model(...)`, `torch.*`, or `tensor.*`), inside a `for` body. Counter-pattern: accumulate a `list` of tensors and stack-then-transfer once.
- **Tier**: Tier B (very high precision when combined with the import set `import torch`).

### 5.2 Missing `model.eval()` / `torch.no_grad()` / `torch.inference_mode()`

Forgetting `model.eval()` leaves dropout active and BN in training mode; forgetting `torch.no_grad()` keeps autograd state alive (higher memory + slower) ([PyTorch Forums: model.eval vs no_grad](https://discuss.pytorch.org/t/model-eval-vs-with-torch-no-grad/19615), [OpenIllumi: PyTorch Inference Fix](https://openillumi.com/en/en-pytorch-inference-mode-no-grad-eval/)). Modern guidance: use `torch.inference_mode()` which subsumes `no_grad` ([PyTorch Forums: no_grad vs inference_mode](https://discuss.pytorch.org/t/pytorch-torch-no-grad-vs-torch-inference-mode/134099)).

- **Tier B shape**: a function named `predict`/`infer`/`forward`/`generate`/`classify`/`embed` (or under a handler decorator) that calls `model(x)` without an enclosing `with torch.no_grad():` / `with torch.inference_mode():` and without a `model.eval()` somewhere in the enclosing scope.
- **Tier**: Tier B with the name-heuristic, Tier C without.

### 5.3 `optimizer.zero_grad(set_to_none=False)`

PyTorch 1.7+ recommends `set_to_none=True` (default since 2.0). Tier-D one-liner: `zero_grad(set_to_none=False)`.

### 5.4 DataLoader defaults

`num_workers=0` (single-process loading) and `pin_memory=False` combined with CUDA are the headline DataLoader perf bug ([Speed Up Model Training — PyTorch Lightning docs](https://lightning.ai/docs/pytorch/stable/advanced/speed.html), [pin_memory guide — PyTorch Tutorials](https://docs.pytorch.org/tutorials/intermediate/pinmem_nonblock.html), [Hey Amit: When to Set pin_memory to True](https://medium.com/data-scientists-diary/when-to-set-pin-memory-to-true-in-pytorch-75141c0f598d)). Also: `persistent_workers=False` causes worker re-spawn each epoch.

- **Tier B shape**: `DataLoader(dataset, ...)` where neither `num_workers=` nor `pin_memory=` are set, AND `torch.cuda.is_available()` / `device="cuda"` is present elsewhere in the file. Counter-pattern: `num_workers>0, pin_memory=True, persistent_workers=True, .to(device, non_blocking=True)`.
- **Tier**: Tier B.

### 5.5 Tensor allocation per iter in training loop

`x = torch.zeros(...)` inside `for batch in loader:` instead of preallocated buffer. Hard to detect cleanly without intra-procedural alias analysis — Tier-C.

### 5.6 `torch.compile` not used (PyTorch 2.0+)

Average 2.24× speedup on inference when used correctly; but performance can *regress* under `torch.inference_mode` unless compilation happens inside that context ([PyTorch Forums: torch.compile under inference_mode](https://discuss.pytorch.org/t/performance-of-torch-compile-is-significantly-slowed-down-under-torch-inference-mode/191939), [PyTorch issue #114119](https://github.com/pytorch/pytorch/issues/114119)). Surface as Tier-D advisory only — many real reasons not to compile.

### 5.7 `pandas.iterrows()` / `.apply(axis=1)` in hot path

Benchmark: 1M rows, regular `for` 187s, `iterrows()` 65.5s, `.apply` 6.8s, vectorised 0.076s ([Stop Writing Slow Pandas Code — DZone](https://dzone.com/articles/stop-slow-pandas-code-vectorization-polars-duckdb), [4 Pandas Anti-Patterns to Avoid — Aidan Cooper](https://www.aidancooper.co.uk/pandas-anti-patterns/)). Polars' lazy mode is the recommended escape for >5M rows.

- **Tier B shape**: `df.iterrows()` or `df.itertuples()` inside a loop body that performs arithmetic on the row, OR `df.apply(lambda row: ..., axis=1)`. Imports: `import pandas as pd`.
- **Tier**: Tier B for `iterrows`; Tier C for `.apply(axis=1)` (sometimes legitimate for object columns).

### 5.8 `sklearn.fit` inside an inference handler

Trivial detection; rarely seen in production but does appear in glue code.

- **Tier B shape**: `<estimator>.fit(...)` where the receiver type is inferable as a sklearn estimator (import set: `from sklearn`), inside a handler function.

### 5.9 `from_pretrained` / `safetensors` load per request

Same shape as 1.8 / 2.1.

- **Tier B shape**: `AutoModelForCausalLM.from_pretrained(...)`, `AutoModel.from_pretrained(...)`, `pipeline(...)`, `safetensors.torch.load_file(...)`, `torch.load(...)` inside a handler. Import set: `from transformers import`, `from safetensors import`, `import torch`.
- **Tier**: Tier B.

### 5.10 JAX recompilation

Marking frequently-changing values as `static_argnames` retraces every call, negating `jit` ([JAX JIT compilation docs](https://docs.jax.dev/en/latest/jit-compilation.html), [Avoiding JAX Recompilation — apxml](https://apxml.com/courses/advanced-jax/chapter-2-optimizing-jax-code-performance/avoiding-recompilation)).

- **Tier C shape**: `@jax.jit(static_argnames=("seed",))` or `static_argnames` listing a parameter that's clearly per-request (named `request_id`, `user_id`, etc.). Heuristic — name-pattern-based.

---

## 6. Inference Serving Patterns (vLLM / TGI / TensorRT / Triton / Ollama / llama.cpp)

These tend to be **configuration anti-patterns** rather than call-site bugs, so the detector becomes a launch-arg / YAML scan, not a tree-sitter Python pass.

### 6.1 Per-request model load (Ollama / llama.cpp)

Documented: first request loads, subsequent are instant; `OLLAMA_NUM_PARALLEL` (default 1 historically) controls concurrent slot count; `OLLAMA_MAX_LOADED_MODELS` (default 1) governs LRU swap ([Why Ollama and llama.cpp crawl — popularai.org](https://www.popularai.org/p/why-ollama-and-llama-cpp-crawl-when-models-spill-into-ram-and-how-to-fix-it), [llama-server keeps loading and unloading — ggml-org discussion #12800](https://github.com/ggml-org/llama.cpp/discussions/12800)). Drift's `docker.rs` already inspects Dockerfile/compose — extend it to flag missing `OLLAMA_NUM_PARALLEL` env var with `>= 4` value for production images.

### 6.2 vLLM `--max-num-seqs` too low / `--gpu-memory-utilization` left at default

`--max-num-seqs` default 1024 in V1; raise to 2048+ for high-traffic; `--gpu-memory-utilization` default 0.90, push to 0.95 on bare metal ([How to Configure vLLM for LLM Serving — OneUptime](https://oneuptime.com/blog/post/2026-01-25-vllm-llm-serving/view), [vLLM Throughput Guide — easecloud](https://blog.easecloud.io/ai-cloud/increase-throughput-with-vllm-serving/)). PagedAttention is always-on in vLLM, but `--quantization awq|gptq|fp8` must match the weights, and `--max-model-len` too high starves the KV pool.

- **Detector**: extend `docker.rs` / Helm-chart scan. Tier B against literal CLI args.

### 6.3 TGI / Triton config

Same shape (`--max-batch-prefill-tokens`, `--max-concurrent-requests`). Out of tree-sitter scope; YAML detector.

---

## 7. Multimodal (images / audio / video)

### 7.1 PIL `Image.open` → `.resize` → `.save` → reopen

The Pillow docs document `thumbnail()` as the optimized path — it calls `draft()` internally and does two-step resampling ([Pillow Image module docs](https://pillow.readthedocs.io/en/stable/reference/Image.html)). The anti-pattern is *manually* doing `Image.open(p).resize(...).save(p); reopen(p)` to round-trip through disk.

- **Tier C shape**: a sequence of `.open(p)` → `.resize(...)` → `.save(p)` followed by another `.open(p)` for the same `p` literal. Hard to do without basic dataflow; emit advisory.
- **Counter-pattern**: `img = Image.open(p); img.thumbnail((w,h)); img.save(out)` — no round-trip.

### 7.2 Image decode per request without cache

- **Tier C shape**: `Image.open(...)` inside a handler with no surrounding `@lru_cache` or filesystem-mirror cache key.

### 7.3 Audio resample per call

- **Tier C shape**: `librosa.load(p, sr=16000)` or `torchaudio.load(...)` followed by `torchaudio.transforms.Resample(...)` inside a handler. Pre-converted assets are the fix.

### 7.4 Whisper full-file vs chunked streaming

Default Whisper expects ≤30s chunks; long files need VAD-aware chunking with 2-3s overlap. Fixed-window naive chunking splits words ([whisper_streaming — ufal/whisper_streaming](https://github.com/ufal/whisper_streaming), [Whisper Audio Chunking — saytowords](https://www.saytowords.com/en/blogs/Whisper-Audio-Chunking/), [whisper-large-v2 chunking discussion](https://huggingface.co/openai/whisper-large-v2/discussions/67)). Padding with zeros causes hallucinations on short chunks.

- **Tier C shape**: `whisper.transcribe(audio)` (full file) or `pipeline("automatic-speech-recognition")(file)` without `chunk_length_s=` or `stride_length_s=` kwargs. Imports: `import whisper`, `from transformers import pipeline`.

### 7.5 OpenCV inefficient ops

`cv2.cvtColor` repeated, `cv2.imread` per request — same shape family as PIL.

---

## 8. Cost / Observability Heuristics Worth Surfacing

### 8.1 Token-cost estimator per call

When the model name is a literal string, drift can multiply by a hard-coded $/1K-token table. Embed a small JSON sheet keyed by model; pin per release. Surface as `estimated_cost_per_call: $X.YY` next to the call site. Maps directly onto OpenTelemetry's `gen_ai.usage.input_tokens` / `gen_ai.usage.output_tokens` semantic conventions so the static prediction can be reconciled against the trace ([OpenTelemetry GenAI semantic conventions](https://opentelemetry.io/docs/specs/semconv/gen-ai/), [GenAI client spans](https://opentelemetry.io/docs/specs/semconv/gen-ai/gen-ai-spans/), [GenAI metrics](https://opentelemetry.io/docs/specs/semconv/gen-ai/gen-ai-metrics/)).

### 8.2 "Hot prompt" detection

A static string literal passed to `messages=` / `system=` seen in >1 file is a *prefix cache candidate*. Drift's existing import-graph can already do this: hash all literal string contents > 500 chars; if the same hash appears in ≥2 call sites, emit "shared prompt — consider centralising and enabling prompt caching".

### 8.3 Long prompts (>8k tokens) without cache

Cheap proxy: length of all concatenated string literals in `system=`/`messages=` > ~30,000 chars (~8K tokens). Emit advisory: prompt is large; enable cache_control (Anthropic) or rely on automatic prefix cache (OpenAI) and ensure prefix stability.

### 8.4 Cache-miss-by-design chat construction

`messages=[{"role":"system","content": f"User {user_id}..."}, ...]` — a per-user f-string in the *first* message tanks prefix caching for everyone. Detector: any f-string interpolation inside the first element of a `messages=` list literal is a Tier-B finding.

---

## 9. Existing Static Analyzers / Lint Plugins (the state of the art)

Bottom line: **there are essentially no first-party static linters that catch the patterns in §1–§8.** Coverage is almost entirely runtime/observability, leaving drift-static-profiler a wide-open niche.

### 9.1 Ruff / Pyright / Mypy

Ruff (Apache-2.0/MIT) has 900+ built-in rules but explicitly does **not** support custom plugins — popular Flake8 plugins are re-implemented in Rust as part of Ruff itself ([Ruff FAQ](https://docs.astral.sh/ruff/faq/), [astral-sh/ruff](https://github.com/astral-sh/ruff)). No AI-specific rule families exist as of search-time. Pyright/Mypy are type-checkers; they catch the wrong-kwarg version of §1.10/§1.11 (Anthropic `max_tokens` missing) if the SDK ships strict overloads, but nothing semantic.

### 9.2 Sentry AI Agent Monitoring (runtime, not static)

Sentry auto-instruments OpenAI, Anthropic, Google GenAI, LangChain, LangGraph, Pydantic AI, OpenAI Agents SDK, and Vercel AI SDK ([Sentry AI Agent Observability blog](https://blog.sentry.io/ai-agent-observability-developers-guide-to-agent-monitoring/), [Sentry AI and LLM Observability](https://sentry.io/solutions/ai-observability/)). Surfaces model calls, tool selections, decision chains, token consumption, costs, latency spikes, budget overruns. **Pure runtime — drift complements, doesn't compete.**

### 9.3 OpenTelemetry GenAI Semantic Conventions

The GenAI SIG ratified semconv covering `gen_ai.system`, `gen_ai.request.model`, `gen_ai.usage.input_tokens`, `gen_ai.usage.output_tokens`, plus `cache_creation.input_tokens` / `cache_read.input_tokens` ([OpenTelemetry GenAI semconv](https://opentelemetry.io/docs/specs/semconv/gen-ai/), [GenAI agent and framework spans](https://opentelemetry.io/docs/specs/semconv/gen-ai/gen-ai-agent-spans/), [GenAI events](https://opentelemetry.io/docs/specs/semconv/gen-ai/gen-ai-events/)). Drift should *emit* these conventions in its category metadata so downstream observability can correlate static findings with traces — Datadog, OneUptime, OpenObserve, Traceloop, and Sentry all consume them ([Datadog: LLM Observability natively supports OpenTelemetry GenAI](https://www.datadoghq.com/blog/llm-otel-semantic-convention/), [Inside the LLM Call: GenAI Observability with OpenTelemetry](https://opentelemetry.io/blog/2026/genai-observability/)).

### 9.4 OpenLLMetry / Traceloop

OSS (Apache-2.0) instrumentations that wrap OpenAI/Anthropic/LangChain. Runtime-only. Best as a reference catalog of which call paths matter ([OpenLLMetry Sentry integration](https://www.traceloop.com/docs/openllmetry/integrations/sentry)).

### 9.5 Vendor cookbook anti-pattern callouts

- **OpenAI Cookbook** — `How_to_handle_rate_limits` notebook ([openai-cookbook example](https://github.com/openai/openai-cookbook/blob/main/examples/How_to_handle_rate_limits.ipynb)) lists exponential backoff with tenacity as canonical. No anti-pattern linter, but the notebooks are a high-quality rule mining source.
- **Anthropic Cookbook** — `prompt_caching.ipynb` ([anthropics/anthropic-cookbook](https://github.com/anthropics/anthropic-cookbook/blob/main/misc/prompt_caching.ipynb)) explicitly walks through wrong vs right `cache_control` placement.
- **Pinecone / Qdrant / Weaviate docs** — each lists batch upsert size recommendations.
- **PyTorch official perf tutorials** — `pinmem_nonblock.html` is the canonical reference for §5.4 ([Pinned memory tutorial](https://docs.pytorch.org/tutorials/intermediate/pinmem_nonblock.html)).

### 9.6 LangChain / LlamaIndex lint?

No first-party lint tool. The closest things are LangSmith (runtime tracing), LangChain Evaluators (eval-quality, not perf), and the deprecation warnings emitted at import time. The `LLMChain` deprecation is documented at [LangChain v0.2 deprecations](https://python.langchain.com/v0.2/docs/versions/v0_2/deprecations/), enforceable as a Tier-B static rule today.

### 9.7 Other ecosystem signals

- LiteLLM ships a cost map for ~100 models — useful as a drop-in for §8.1 ([BerriAI/litellm issue #11364](https://github.com/BerriAI/litellm/issues/11364)).
- Pydantic-AI's discussion of `max_tokens` defaults (§1.10) is one of the few places anyone has documented the runaway-default trap ([pydantic-ai #2553](https://github.com/pydantic/pydantic-ai/issues/2553)).

---

## Suggested Catalog Additions to `tags.rs` / `categories.rs`

Concrete proposal:

1. Add `Category::Llm` and `Category::Vector` to `Category`. Don't fold into `Network` — the cost/UX semantics are different enough to warrant distinct insight rendering.
2. Extend `classify_module` with the import table below (each maps to one category):
   - `Llm`: `openai`, `anthropic`, `cohere`, `mistralai`, `google.genai`, `groq`, `together`, `replicate`, `fireworks.client`, `litellm`, `instructor`, `langchain_openai`, `langchain_anthropic`, `langchain_google_genai`, `langchain_cohere`.
   - `Vector`: `pinecone`, `qdrant_client`, `weaviate`, `pymilvus`, `chromadb`, `faiss`, `usearch`, `lancedb`.
   - `Compute` (already exists): `torch`, `tensorflow`, `jax`, `sklearn`, `transformers`, `sentence_transformers`, `fastembed`, `numpy` (with caveats), `pandas`, `polars`.
3. Add `classify_receiver_pattern` entries: `llm`, `openai_client`, `anthropic_client`, `embed_model`, `embedder`, `index`, `vector_store`, `retriever`, `chain`, `agent`, `pipeline`, `kernel`.
4. Add an `Insight` rule type `Insight::LlmInLoop`, `Insight::EmbeddingInLoop`, `Insight::VectorUpsertInLoop`, `Insight::ModelLoadInHandler`, `Insight::TokenizerInHandler`, `Insight::SyncLlmInAsync`, `Insight::MissingCacheControl`, `Insight::DeprecatedLLMChain`, `Insight::ItemInLoop`, `Insight::IterrowsInLoop`, `Insight::DataLoaderDefaults`, `Insight::PgvectorOperatorMismatch` — each maps to the AST shapes listed above.
5. Ship a small `models_cost.json` keyed by model literal (e.g. `gpt-4o`, `claude-3-5-sonnet-20241022`) → `(input_per_1k, output_per_1k, cached_input_per_1k)` so the cost estimator in §8.1 has data. Refresh quarterly.

The N+1 detector you already have for ORMs *is* the prototype for §1.5, §1.9, §3.1, §3.7, and §4.1 — the loop-body shape match generalizes almost verbatim; only the import table grows.

---

### Files I touched / referenced

- `/Users/ilyashusterman/Projects/drift/drift-static-profiler/src/categories.rs` — taxonomy extension target.
- `/Users/ilyashusterman/Projects/drift/drift-static-profiler/src/tags.rs` — tag-emitter target.
- `/Users/ilyashusterman/Projects/drift/drift-static-profiler/src/insights.rs` — new `Insight::*` variants land here.
- `/Users/ilyashusterman/Projects/drift/drift-static-profiler/src/docker.rs` — extend for §6 vLLM/Ollama env-var scan.
- `/Users/ilyashusterman/Projects/drift/drift-static-profiler/research/ORM_NICHE_CATALOG.md` — format precedent followed above.

### Sources

- [Anthropic Prompt Caching — Claude API Docs](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)
- [anthropics/anthropic-cookbook prompt_caching.ipynb](https://github.com/anthropics/anthropic-cookbook/blob/main/misc/prompt_caching.ipynb)
- [dev.to: I Was Caching Wrong This Whole Time](https://dev.to/yurukusa/i-was-caching-wrong-this-whole-time-anthropic-academy-part-1-1hba)
- [PromptHub: Prompt Caching with OpenAI, Anthropic, Google](https://www.prompthub.us/blog/prompt-caching-with-openai-anthropic-and-google-models)
- [DigitalOcean: Prompt Caching for Anthropic & OpenAI](https://www.digitalocean.com/blog/prompt-caching-with-digital-ocean)
- [DeepWiki: openai-python Client Configuration](https://deepwiki.com/openai/openai-python/2.1-client-configuration)
- [groq/groq-python](https://github.com/groq/groq-python)
- [OpenAI Agents SDK config](https://openai.github.io/openai-agents-python/config/)
- [Embeddings API Max Batch Size — OpenAI Community](https://community.openai.com/t/embeddings-api-max-batch-size/655329)
- [OpenAI Embeddings API reference](https://platform.openai.com/docs/api-reference/embeddings/create)
- [DeepWiki: tiktoken Caching Strategy](https://deepwiki.com/openai/tiktoken/5.1-caching-strategy-and-configuration)
- [openai/tiktoken](https://github.com/openai/tiktoken)
- [The Concurrency Mistake Hiding in Every FastAPI AI Service](https://jamwithai.substack.com/p/the-concurrency-mistake-hiding-in)
- [Async/Await Isn't Enough: Solving Synchronous Bottlenecks in FastAPI](https://medium.com/@aryanrot234/async-await-isnt-enough-solving-synchronous-bottlenecks-in-fastapi-6f9f152256a2)
- [10 Async Pitfalls in FastAPI](https://medium.com/@bhagyarana80/10-async-pitfalls-in-fastapi-and-how-to-avoid-them-60d6c67ea48f)
- [Anthropic Streaming messages](https://docs.anthropic.com/en/docs/build-with-claude/streaming)
- [pydantic-ai issue #2553 — Anthropic max_tokens default](https://github.com/pydantic/pydantic-ai/issues/2553)
- [openai-cookbook How_to_handle_rate_limits](https://github.com/openai/openai-cookbook/blob/main/examples/How_to_handle_rate_limits.ipynb)
- [OpenAI Cookbook: How to handle rate limits](https://cookbook.openai.com/examples/how_to_handle_rate_limits)
- [Introducing Structured Outputs in the API — OpenAI](https://openai.com/index/introducing-structured-outputs-in-the-api/)
- [Structured model outputs guide](https://developers.openai.com/api/docs/guides/structured-outputs)
- [sentence-transformers Issue #2551 — set a reasonable default batch_size](https://github.com/huggingface/sentence-transformers/issues/2551)
- [Medium: Correctly Set Batch Size in Sentence Transformers](https://medium.com/@vici0549/it-is-crucial-to-properly-set-the-batch-size-when-using-sentence-transformers-for-embedding-models-3d41a3f8b649)
- [sentence-transformers Issue #2312 — batch-size-affects-embedding](https://github.com/UKPLab/sentence-transformers/issues/2312)
- [Cohere Rerank API reference](https://docs.cohere.com/reference/rerank)
- [Cohere Rerank Best Practices](https://docs.cohere.com/docs/reranking-best-practices)
- [Pinecone LlamaIndex integration](https://docs.pinecone.io/integrations/llamaindex)
- [FAISS brute-force search wiki](https://github.com/facebookresearch/faiss/wiki/Brute-force-search-without-an-index)
- [USearch on Hacker News](https://news.ycombinator.com/item?id=36942993)
- [AWS: pgvector indexing deep dive into IVFFlat and HNSW](https://aws.amazon.com/blogs/database/optimize-generative-ai-applications-with-pgvector-indexing-a-deep-dive-into-ivfflat-and-hnsw-techniques/)
- [DeepWiki: pgvector creating and using indexes](https://deepwiki.com/pgvector/pgvector/5.2-creating-and-using-indexes)
- [LangChain RunnableSequence API](https://api.python.langchain.com/en/latest/runnables/langchain_core.runnables.base.RunnableSequence.html)
- [LangChain v0.2 deprecations (LLMChain)](https://python.langchain.com/v0.2/docs/versions/v0_2/deprecations/)
- [LCEL explained — Pinecone](https://www.pinecone.io/learn/series/langchain/langchain-expression-language/)
- [Why Parallel Tool Calling Matters for LLM Agents — CodeAnt](https://www.codeant.ai/blogs/parallel-tool-calling)
- [JS LangChain: How to disable parallel tool calling](https://js.langchain.com/docs/how_to/tool_calling_parallel/)
- [LlamaIndex Persisting & Loading Data](https://docs.llamaindex.ai/en/stable/module_guides/storing/save_load)
- [PyTorch Forums: Tensor.item() takes a lot of running time](https://discuss.pytorch.org/t/tensor-item-takes-a-lot-of-running-time/16683/21)
- [pytorch/pytorch issue #108968 — unnecessary cuda synchronizations](https://github.com/pytorch/pytorch/issues/108968)
- [PyTorch Forums: CPU-GPU synchronization optimization](https://discuss.pytorch.org/t/performance-optimization-re-cpu-gpu-synchronization/157251)
- [PyTorch Forums: model.eval() vs torch.no_grad()](https://discuss.pytorch.org/t/model-eval-vs-with-torch-no-grad/19615)
- [OpenIllumi: PyTorch Inference Fix](https://openillumi.com/en/en-pytorch-inference-mode-no-grad-eval/)
- [PyTorch Forums: no_grad vs inference_mode](https://discuss.pytorch.org/t/pytorch-torch-no-grad-vs-torch-inference-mode/134099)
- [PyTorch Lightning: Speed Up Model Training](https://lightning.ai/docs/pytorch/stable/advanced/speed.html)
- [Pinned memory tutorial — PyTorch](https://docs.pytorch.org/tutorials/intermediate/pinmem_nonblock.html)
- [When to set pin_memory to True](https://medium.com/data-scientists-diary/when-to-set-pin-memory-to-true-in-pytorch-75141c0f598d)
- [PyTorch Forums: torch.compile under inference_mode regression](https://discuss.pytorch.org/t/performance-of-torch-compile-is-significantly-slowed-down-under-torch-inference-mode/191939)
- [pytorch/pytorch issue #114119 — torch.compile + inference_mode](https://github.com/pytorch/pytorch/issues/114119)
- [Stop Writing Slow Pandas Code — DZone](https://dzone.com/articles/stop-slow-pandas-code-vectorization-polars-duckdb)
- [4 Pandas Anti-Patterns to Avoid — Aidan Cooper](https://www.aidancooper.co.uk/pandas-anti-patterns/)
- [JAX JIT compilation docs](https://docs.jax.dev/en/latest/jit-compilation.html)
- [Avoiding JAX Recompilation — apxml](https://apxml.com/courses/advanced-jax/chapter-2-optimizing-jax-code-performance/avoiding-recompilation)
- [How to Configure vLLM for LLM Serving — OneUptime](https://oneuptime.com/blog/post/2026-01-25-vllm-llm-serving/view)
- [vLLM Throughput Guide — easecloud](https://blog.easecloud.io/ai-cloud/increase-throughput-with-vllm-serving/)
- [LLM Serving Optimization H100 — Spheron](https://www.spheron.network/blog/llm-serving-optimization-continuous-batching-paged-attention/)
- [vLLM PagedAttention design docs](https://docs.vllm.ai/en/latest/design/paged_attention/)
- [Why Ollama and llama.cpp crawl — popularai.org](https://www.popularai.org/p/why-ollama-and-llama-cpp-crawl-when-models-spill-into-ram-and-how-to-fix-it)
- [llama.cpp loading/unloading discussion #12800](https://github.com/ggml-org/llama.cpp/discussions/12800)
- [Pillow Image module docs](https://pillow.readthedocs.io/en/stable/reference/Image.html)
- [whisper_streaming — ufal](https://github.com/ufal/whisper_streaming)
- [Whisper Audio Chunking — saytowords](https://www.saytowords.com/en/blogs/Whisper-Audio-Chunking/)
- [whisper-large-v2 long-audio chunking discussion](https://huggingface.co/openai/whisper-large-v2/discussions/67)
- [OpenTelemetry GenAI semconv index](https://opentelemetry.io/docs/specs/semconv/gen-ai/)
- [GenAI client spans semconv](https://opentelemetry.io/docs/specs/semconv/gen-ai/gen-ai-spans/)
- [GenAI agent and framework spans semconv](https://opentelemetry.io/docs/specs/semconv/gen-ai/gen-ai-agent-spans/)
- [GenAI metrics semconv](https://opentelemetry.io/docs/specs/semconv/gen-ai/gen-ai-metrics/)
- [GenAI events semconv](https://opentelemetry.io/docs/specs/semconv/gen-ai/gen-ai-events/)
- [Datadog: native OTel GenAI semconv support](https://www.datadoghq.com/blog/llm-otel-semantic-convention/)
- [Inside the LLM Call: GenAI Observability with OpenTelemetry](https://opentelemetry.io/blog/2026/genai-observability/)
- [Sentry AI Agent Observability blog](https://blog.sentry.io/ai-agent-observability-developers-guide-to-agent-monitoring/)
- [Sentry AI and LLM Observability](https://sentry.io/solutions/ai-observability/)
- [OpenLLMetry Sentry integration — Traceloop](https://www.traceloop.com/docs/openllmetry/integrations/sentry)
- [Ruff FAQ](https://docs.astral.sh/ruff/faq/)
- [astral-sh/ruff](https://github.com/astral-sh/ruff)
- [IBM: What is Prompt Caching?](https://www.ibm.com/think/topics/prompt-caching)
- [BerriAI/litellm cost issue #11364](https://github.com/BerriAI/litellm/issues/11364)
- [Groq Batch API docs](https://console.groq.com/docs/batch)
- [Fireworks AI Python client API reference](https://docs.fireworks.ai/tools-sdks/python-client/api-reference)agentId: a72e53147f455c40c (use SendMessage with to: 'a72e53147f455c40c' to continue this agent)
<usage>total_tokens: 97292
tool_uses: 44
duration_ms: 488750</usage>