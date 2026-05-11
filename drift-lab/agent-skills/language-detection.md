# Language detection playbook

Identifying what a project is written in shouldn't cost more than 2-3 tool
calls. Follow this order — each step gives stronger signal than the next.

## 1. Read the Dockerfile FIRST (highest signal)

If `find_image` returned a `manifest_path`, the very next tool call should be
`read_file_excerpt` on that path. The `FROM` line answers the language
question in one read:

| Base image                  | Language / runtime         |
|-----------------------------|----------------------------|
| `python:3.x`, `python:slim` | Python (version follows)   |
| `node:20`, `node:lts`       | Node.js / TypeScript       |
| `oven/bun:1`                | Bun (TypeScript)           |
| `rust:1.78`, `rust:slim`    | Rust                       |
| `golang:1.22`               | Go                         |
| `openjdk:21`, `eclipse-temurin` | JVM (Java/Kotlin/Scala) |
| `ruby:3.3`                  | Ruby                       |
| `php:8.x`                   | PHP                        |
| `mcr.microsoft.com/dotnet/*` | .NET                      |
| `elixir:1.x`                | Elixir                     |

The `COPY`, `RUN`, and `CMD` lines reveal the package manager and entrypoint
(`pip install` / `npm ci` / `cargo build` / `go mod download` / `mvn package`).
That's usually all you need.

If the FROM is generic (`alpine`, `debian`, `ubuntu`, `scratch`), drop down
to step 2 — but first scan the rest of the Dockerfile for binaries being
installed (`apt-get install python3`, `apk add nodejs`, etc.).

## 2. Check manifest files (next-best signal)

`list_directory` on the project root. The presence of a manifest pins the
language unambiguously:

| File present                                                | Language          |
|-------------------------------------------------------------|-------------------|
| `package.json`                                              | Node.js / TS      |
| `bun.lockb`, `bunfig.toml`                                  | Bun (TS)          |
| `pyproject.toml`, `requirements.txt`, `Pipfile`, `setup.py` | Python            |
| `Cargo.toml`                                                | Rust              |
| `go.mod`                                                    | Go                |
| `pom.xml`                                                   | Java (Maven)      |
| `build.gradle`, `build.gradle.kts`, `settings.gradle*`      | Java/Kotlin       |
| `Gemfile`, `*.gemspec`                                      | Ruby              |
| `composer.json`                                             | PHP               |
| `*.csproj`, `*.fsproj`, `*.sln`                             | .NET              |
| `mix.exs`                                                   | Elixir            |
| `Package.swift`                                             | Swift             |

When multiple manifests coexist (e.g. `package.json` + `pyproject.toml`),
this is a **polyglot** project — the Dockerfile decides which runtime
actually runs. Re-read the Dockerfile's `CMD` / `ENTRYPOINT`.

The `discover_project` tool already scans for these. Run it once at the
start; only re-run if its result is ambiguous.

## 3. File-extension counts (sanity check only)

Counting `.py` / `.ts` / `.rs` / `.go` / `.java` etc. via `list_directory`
can *confirm* step 2 but should **never** be your primary signal —
configuration snippets in any language can sit next to a project's main code
(e.g. a Python project with a `.ts` config for an admin dashboard). Use this
only to break ties.

## 4. Imports / dependencies (for FRAMEWORK detection)

Once the language is known, peek at imports to identify the FRAMEWORK —
that's what determines profiler attach strategy and which endpoints to load:

* **Python**: read `requirements.txt` or `pyproject.toml [project].dependencies` for
  `fastapi`, `flask`, `django`, `starlette`, `aiohttp`, `tornado`, `sanic`.
* **Node**: read `package.json` `dependencies` / `devDependencies` for
  `express`, `fastify`, `nestjs` / `@nestjs/core`, `next`, `koa`, `hapi`.
* **JVM**: scan `pom.xml` / `build.gradle` for
  `spring-boot-starter`, `quarkus`, `micronaut`, `dropwizard`, `ktor`.
* **Go**: grep entrypoint imports for `gin`, `echo`, `chi`, `gorilla/mux`,
  `fiber`, `net/http` (stdlib).
* **Rust**: read `Cargo.toml [dependencies]` for `actix-web`, `axum`,
  `rocket`, `warp`, `tide`.
* **Ruby**: `Gemfile` for `rails`, `sinatra`, `roda`, `hanami`.
* **PHP**: `composer.json` for `laravel/framework`, `symfony/*`, `slim/slim`.

For one quick read use `read_file_excerpt` on the manifest with
`max_lines: 80` — that's enough to see top-level deps without paging the
whole file.

## 5. When you genuinely can't tell

If you've made 3+ tool calls and STILL can't pin the language, stop. Don't
keep speculating. Report verbatim:

> "Unable to determine language. Tried Dockerfile (status), manifests (list),
> top-level files: …. Recommend the user add a Dockerfile or specify the
> runtime."

Then end the run — calling `detect_runtime` / `install_profiler` blind will
just fail with cryptic errors that confuse the user.

## Anti-patterns — actively avoid

* **Don't crawl** the directory tree. One `list_directory` at the root
  surfaces every manifest you need. Reading 8 subdirectories is wasted
  budget.
* **Don't infer language from the project NAME** — `payment-service` can be
  anything.
* **Don't trust the BASE image alone** when the Dockerfile installs a
  different runtime later. `FROM debian:bookworm` + `RUN apt-get install
  python3.11` is a Python project.
* **Don't run `detect_runtime` before reading the Dockerfile.**
  `detect_runtime` inspects the image's environment (PATH, entrypoint) which
  works in most cases but is slower and less specific than the Dockerfile
  itself. Use it as a fallback, not a starting point.
* **Don't confuse build-time and run-time languages.** A project may use
  TypeScript at build but ship pure JS in the image; a Python project may
  bundle a compiled Cython extension. The language you care about is the
  one whose hot path you'll profile — i.e. the one the `CMD` invokes.
