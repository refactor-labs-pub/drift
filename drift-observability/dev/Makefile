.PHONY: up down reset-data nuke wipe-trace

INFRA_DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
SHARED_VOLUME_PATH := /tmp/drift-trace
ARGS      ?=
ws          ?=
env         ?=
persist ?=

ifneq ($(ws),)
  ARGS += $(foreach w,$(ws),--workspaces $(w))
endif
ROOT_ENV  := $(INFRA_DIR)../.env
INFRA_ENV := $(INFRA_DIR).env
ENV_FILE  := $(or $(env),$(wildcard $(ROOT_ENV)),$(wildcard $(INFRA_ENV)))
ifneq ($(ENV_FILE),)
  ARGS += --env-file $(ENV_FILE)
endif
ifneq ($(persist),)
  ARGS += --persistence
endif

# example `make up ws=<WORKSPACE-FOLDER-NAME> env=<ROUTE-TO-ENV-FILE>/.env persist=true`
up:
	@docker info > /dev/null 2>&1 || (echo "ERROR: Docker Desktop is not running. Start it and try again." && exit 1)
	@docker pull gcr.io/k8s-minikube/kicbase:v0.0.50
	@minikube status --format="{{.Host}}" 2>/dev/null | grep -q Running || minikube start --memory=8192 --cpus=10 --kubernetes-version=v1.35.1
	@kubectl config use-context minikube
	@pkill -f "tilt up" 2>/dev/null || true
	@$(MAKE) -f $(INFRA_DIR)Makefile wipe-trace
	@if [ -n "$(ENV_FILE)" ]; then \
	  echo "Using env file: $(ENV_FILE)$(if $(env), (override via env=),$(if $(wildcard $(ROOT_ENV)), (root default), (infrastructure default)))"; \
	else \
	  echo "No env file found (looked at $(ROOT_ENV) and $(INFRA_ENV)); pass env=<path> to specify one."; \
	fi
	@echo "Running: cd $(INFRA_DIR) && tilt up --debug -- $(ARGS)"
	cd $(INFRA_DIR) && tilt up --debug -- $(ARGS)

down:
	@pkill -f "tilt up" 2>/dev/null || true
	@if minikube status --format="{{.Host}}" 2>/dev/null | grep -q Running; then \
	  if kubectl config get-contexts -o name 2>/dev/null | grep -qx minikube; then \
	    kubectl config use-context minikube >/dev/null; \
	  fi; \
	  (cd $(INFRA_DIR) && tilt down --debug) || echo "warning: tilt down exited non-zero, continuing"; \
	else \
	  echo "minikube not running — skipping tilt down (no resources to tear down)"; \
	fi
	@$(MAKE) -f $(INFRA_DIR)Makefile wipe-trace

# Removes accumulated trace events on the minikube node so the next
# `make up` starts with an empty /trace/events.log.
wipe-trace:
	@if minikube status --format="{{.Host}}" 2>/dev/null | grep -q Running; then \
	  echo "Wiping shared trace volume on minikube node: $(SHARED_VOLUME_PATH)"; \
	  minikube ssh -- "sudo rm -rf $(SHARED_VOLUME_PATH)/* 2>/dev/null || true"; \
	else \
	  echo "minikube not running, skipping wipe"; \
	fi

stop:
	@minikube stop

reset-data:
	@if ! minikube status --format="{{.Host}}" 2>/dev/null | grep -q Running; then \
	  echo "minikube not running — start it first with 'make up' (cannot delete PVCs against an offline cluster)"; \
	  exit 1; \
	fi
	@kubectl config use-context minikube >/dev/null
	@echo "Deleting all PersistentVolumeClaims in 'local' namespace..."
	@kubectl delete pvc --all -n local --ignore-not-found
	@echo "Done. Run 'make up' to start fresh with empty databases."

nuke:
	@pkill -f "tilt up" 2>/dev/null || true
	@if minikube status --format="{{.Host}}" 2>/dev/null | grep -q Running; then \
	  if kubectl config get-contexts -o name 2>/dev/null | grep -qx minikube; then \
	    kubectl config use-context minikube >/dev/null; \
	  fi; \
	  (cd $(INFRA_DIR) && tilt down --debug) || echo "warning: tilt down exited non-zero, continuing to minikube delete"; \
	else \
	  echo "minikube not running — skipping tilt down, going straight to minikube delete"; \
	fi
	@minikube delete || echo "warning: minikube delete exited non-zero (cluster may already be gone)"

install-minikube:
	@brew install minikube kubectl

install-tilt-helm-k9s:
	@brew install tilt-dev/tap/tilt helm k9s

.PHONY: validate-docker-ram
validate-docker-ram:
	@docker info > /dev/null 2>&1 || (echo "ERROR: Docker Desktop is not running. Start it and try again." && exit 1)
	@MEM_BYTES=$$(docker info --format '{{.MemTotal}}' 2>/dev/null); \
	if [ -z "$$MEM_BYTES" ] || [ "$$MEM_BYTES" = "0" ]; then \
		echo "⚠️  Could not determine Docker RAM allocation. Ensure Docker Desktop has at least 12GB allocated."; \
		exit 1; \
	fi; \
	MEM_GB=$$(awk -v b=$$MEM_BYTES 'BEGIN { printf "%.2f", b/1024/1024/1024 }'); \
	OK=$$(awk -v b=$$MEM_BYTES 'BEGIN { print (b >= 12*1024*1024*1024) ? "1" : "0" }'); \
	if [ "$$OK" = "1" ]; then \
		echo "✓ Docker RAM:     $${MEM_GB} GiB (>= 12 GiB)"; \
	else \
		echo "✗ Docker RAM:     $${MEM_GB} GiB (need >= 12 GiB)"; \
		echo "   Increase Docker Desktop memory allocation: Settings → Resources → Memory"; \
		exit 1; \
	fi

.PHONY: validate-docker-cpu
validate-docker-cpu:
	@docker info > /dev/null 2>&1 || (echo "ERROR: Docker Desktop is not running. Start it and try again." && exit 1)
	@CPU_COUNT=$$(docker info --format '{{.NCPU}}' 2>/dev/null); \
	if [ -z "$$CPU_COUNT" ] || [ "$$CPU_COUNT" = "0" ]; then \
		echo "⚠️  Could not determine Docker CPU allocation. Ensure Docker Desktop has at least 10 CPUs allocated."; \
		exit 1; \
	fi; \
	OK=$$(awk -v c=$$CPU_COUNT 'BEGIN { print (c >= 10) ? "1" : "0" }'); \
	if [ "$$OK" = "1" ]; then \
		echo "✓ Docker CPUs:    $${CPU_COUNT} (>= 10)"; \
	else \
		echo "✗ Docker CPUs:    $${CPU_COUNT} (need >= 10)"; \
		echo "   Increase Docker Desktop CPU allocation: Settings → Resources → CPUs"; \
		exit 1; \
	fi

.PHONY: validate-docker-resources
validate-docker-resources: validate-docker-ram validate-docker-cpu

.PHONY: install
install: install-minikube install-tilt-helm-k9s validate-docker-ram validate-docker-cpu
	@echo "✓ All tools installed"
	@echo "   - minikube: https://minikube.sigs.k8s.io/docs/start/"
	@echo "   - tilt: https://docs.tilt.dev/install.html"
	@echo "   - helm: https://helm.sh/docs/intro/install/"
	@echo "   - k9s: https://k9scli.io/"
