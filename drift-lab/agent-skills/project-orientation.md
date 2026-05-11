# Project orientation playbook

Before you touch Docker or any profiler, **understand the project's shape**.
A naive `find_image` only checks for `Dockerfile` at the literal root, so on
any monorepo, services-folder layout, or "split prod/dev Dockerfiles" repo
it'll come back empty and you'll waste turns guessing. This playbook tells
you how to find the *canonical* image source — the one CI actually builds
and ships — before you call `find_image`.

Run these steps in order. Don't skip ahead to Docker until step 4.

## 1. Lay of the land — `list_directory` once at the root

One call. From the top-level listing you can already see:

* **Single-service shape** — `Dockerfile`, manifest (`package.json` / `Cargo.toml` /
  `pyproject.toml`), `src/`, `tests/`. `find_image` will Just Work later.
* **Monorepo with workspaces** — `apps/`, `packages/`, `services/`, `pnpm-workspace.yaml`,
  `turbo.json`, `nx.json`, `lerna.json`, `Cargo.toml` with `[workspace]`, top-level
  `pyproject.toml` with `[tool.uv.workspace]`. Each member usually has its own
  Dockerfile.
* **Service-folder layout** — `services/`, `backend/`, `apps/`, `cmd/`, `api/`,
  `web/`. The Dockerfile lives one level deeper.
* **`docker/`, `deploy/`, `infra/`, `ops/` directories** — they often hold the
  *real* Dockerfiles (`docker/api.Dockerfile`, `deploy/Dockerfile.prod`) while
  the root has nothing.
* **Multiple Dockerfiles** at root — `Dockerfile`, `Dockerfile.dev`,
  `Dockerfile.test`. Don't guess which one CI uses; step 2 will tell you.

Note any of these patterns. They drive every later decision.

## 2. CI/CD configs — the source of truth for "which image"

CI pipelines have to explicitly build SOMETHING, so they tell you exactly
which Dockerfile produces the production image. Read these (with
`read_file_excerpt`, `max_lines: 120` is plenty for any of them):

| File / dir                              | What it tells you                                    |
|-----------------------------------------|------------------------------------------------------|
| `.github/workflows/*.yml`               | Look for `docker build -f <path>`, `docker/build-push-action` `file:` |
| `.gitlab-ci.yml`                        | `docker build -f` in `script:` blocks                |
| `.circleci/config.yml`                  | `setup_remote_docker` + build commands               |
| `Jenkinsfile`                           | `sh "docker build -f ..."` lines                     |
| `Makefile`, `makefile`, `justfile`      | `docker-build:` / `build:` / `image:` targets — the `-f` flag is the canonical path |
| `bin/build`, `scripts/build*.sh`, `scripts/docker*.sh`, `bin/release`         | Same — devs put the build command here when it's complex |
| `skaffold.yaml`, `dagger.json`, `earthly` files | Newer build orchestrators — `image:` / `dockerfile:` fields                  |
| `docker-bake.hcl`                       | Buildx bake — `dockerfile` field per target          |
| `.buildkite/`, `.azure-pipelines.yml`   | Same pattern, less common                            |

What to extract from these files:

1. **Dockerfile path** — usually after `-f`, `--file`, `file:`, `dockerfile:`.
2. **Build context** — the `.` or directory at the end of `docker build` or
   `context:` field.
3. **Image tag** — `-t my-org/api:$SHA` or `tags:` — confirms the production
   image name.

If CI builds **multiple images**, pick the one whose name / target matches
the project's primary service (usually obvious from the Makefile target name
or the repo name).

If you find ZERO CI files, that's a strong signal you're in a personal/
demo project — go straight to step 4 with what you have.

## 3. Packages & build tooling — what's actually inside

Once you know which directory the Dockerfile lives in, look at the package
manifest for that service. It tells you the language, framework, dependencies,
and entrypoint. This is what the language-detection playbook covers in
detail — quick checklist:

* `package.json` → JS/TS. Read `scripts.start`, `main`, `dependencies`.
* `pyproject.toml` / `requirements.txt` → Python. Read `[project].dependencies`
  or `[tool.poetry.dependencies]`.
* `Cargo.toml` → Rust. Read `[[bin]]` or `[lib]`, `[dependencies]`.
* `go.mod` → Go. The module name is at the top.
* `pom.xml` / `build.gradle*` → JVM.
* `Gemfile` → Ruby.
* `composer.json` → PHP.

Use `read_file_excerpt` with `max_lines: 80` — top of the file has everything
you need (name, deps, scripts).

If the project is a **monorepo**, also read the root workspace file
(`pnpm-workspace.yaml`, `Cargo.toml [workspace]`, `turbo.json`). This shows
you all the services and which one ships as a container.

## 4. NOW call `find_image`

You've earned the right to call `find_image`. But pass the **right path**:

* If CI told you the Dockerfile is at `services/api/Dockerfile`, call
  `find_image` with `path: "<project_root>/services/api"`.
* If CI told you it's at `docker/Dockerfile.prod`, that's tricky — `find_image`
  expects a `Dockerfile` filename specifically. In that case, **skip
  `find_image`** and use `read_file_excerpt` to inspect the file directly,
  then construct the `image_ref` / `build_context` yourself when calling
  `ensure_image` (the build_context is the project root, the manifest_path is
  the explicit Dockerfile path).
* If CI says nothing and root has nothing, `list_directory` each plausible
  subdir (`apps/`, `services/`, `backend/`, `api/`) and look for Dockerfiles
  there.

If after all this you still can't find a Dockerfile, **stop and report**:

> "No Dockerfile found in `<root>`, `<root>/services/`, `<root>/apps/`,
> `<root>/docker/`. CI configs (X, Y) don't reference one either.
> Recommend the user point at the service subdirectory directly."

Don't keep guessing. That's worse than admitting it.

## Anti-patterns to actively avoid

* **Don't call `find_image` first.** On any non-trivial repo it'll return
  "no Dockerfile" and you'll have no context for what to do next.
* **Don't `list_directory` recursively.** One root listing + targeted reads
  of CI/Makefile is enough. Crawling the tree burns turns without adding
  signal.
* **Don't trust file names** like `Dockerfile.dev` for profiling. Production
  often uses a different file with different layers, optimisations, and
  entrypoint. Read CI to confirm which file ships.
* **Don't conflate workspace root with service root.** In a pnpm /
  Cargo workspace, the root has shared config; each service's manifest +
  Dockerfile sit one directory down.
